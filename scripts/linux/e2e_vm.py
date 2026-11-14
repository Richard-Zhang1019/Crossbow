#!/usr/bin/env python3
"""VM-level Linux TUN/service acceptance on a real host (#427).

Where smoke_network.py proves the broker/core logic inside private namespaces
without systemd, this script drives the production authorization path end to
end on a disposable VM: install-service.sh, a pre-seeded polkit rule standing
in for the desktop password dialog, real `pkexec` authorization, the transient
`crossbow-tun.service` unit, its ExecStopPost recovery after SIGKILL, and
uninstall cleanup. Public data-plane checks need internet; CI runners have no
global IPv6, so the IPv6 data plane stays a documented limitation covered by
the namespace test instead.

Run unprivileged with passwordless sudo (as root nothing would receive
PKEXEC_UID and the authorization under test would be skipped):
    python3 scripts/linux/e2e_vm.py --disposable-vm \
        --service target/release/crossbow-service \
        --core apps/desktop/src-tauri/binaries/mihomo-x86_64-unknown-linux-gnu \
        --output target/linux-vm-e2e
"""
import argparse
from contextlib import ExitStack
import json
import os
import re
from pathlib import Path
import signal
import subprocess
import sys
import time

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from smoke_network import connect, free_port, network_state, request, run, wait  # noqa: E402

SERVICE = '/usr/lib/crossbow/crossbow-service'
CORE = '/usr/lib/crossbow/mihomo'
POLICY = '/usr/share/polkit-1/actions/org.crossbow.tun.policy'
POLKIT_RULE = '/etc/polkit-1/rules.d/49-crossbow-vm-e2e.rules'
INSTALL_DIR = Path(SERVICE).parent
RUNTIME = Path('/run/crossbow-tun')
JOURNAL = RUNTIME / 'network.json'
SOCKET = RUNTIME / 'control.sock'
SESSION_RUNTIME = RUNTIME / 'session/runtime.yaml'
DEVICE = 'Crossbow'
TABLE = '20220'
PRIORITY = '12020'
PROTOCOL = '242'
UNIT = 'crossbow-tun.service'
SECRET = 'crossbow-vm-e2e-0123456789abcdef'
INSTALL_SCRIPT = HERE.parent / 'linux' / 'install-service.sh'

# 与生产连通性探针一致：独立提供方的 HTTPS 204，不依赖共享 runner 的 GitHub API 配额。
HTTPS_PROBES = (
    ('https://www.gstatic.com/generate_204', 'www.gstatic.com'),
    ('https://cp.cloudflare.com/generate_204', 'cp.cloudflare.com'),
)

CHECKS = []


def check(message):
    CHECKS.append(message)
    print(f'[ok] {message}', flush=True)


def sudo(*args):
    return run('sudo', '-n', *args)


def try_run(*args):
    return subprocess.run(args, capture_output=True, text=True, check=False)


def sudo_tee(path, text):
    subprocess.run(['sudo', '-n', 'tee', str(path)], input=text, text=True,
                   check=True, capture_output=True)


def systemctl(properties):
    """Mirror the desktop identity check: one bounded query of the real unit."""
    args = ['systemctl', 'show', UNIT, '--no-pager']
    args += [f'--property={name}' for name in properties]
    return dict(line.split('=', 1) for line in run(*args).splitlines() if '=' in line)


def same_number(value, expected):
    return value is not None and str(value) == str(expected)


def rules_json(family):
    return json.loads(run('ip', '-j', f'-{family}', 'rule', 'show'))


def routes_all(family):
    return json.loads(run('ip', '-j', f'-{family}', 'route', 'show', 'table', 'all'))


def owned_resources_absent():
    for link in json.loads(run('ip', '-j', 'link', 'show')):
        if link['ifname'] == DEVICE:
            return False
    for family in (4, 6):
        for rule in rules_json(family):
            if same_number(rule.get('priority'), PRIORITY) or same_number(rule.get('table'), TABLE):
                return False
        for route in routes_all(family):
            if route.get('dev') == DEVICE or same_number(route.get('table'), TABLE):
                return False
    return True


def service_load_state():
    # systemctl show can return nonzero for a missing unit but still prints its LoadState.
    return try_run('systemctl', 'show', UNIT, '--property=LoadState', '--value').stdout.strip()


def require_clean_host():
    paths = (INSTALL_DIR, POLICY, POLKIT_RULE, RUNTIME)
    existing = [str(path) for path in paths if os.path.lexists(path)]
    if existing or service_load_state() != 'not-found' or not owned_resources_absent():
        raise RuntimeError(f'Crossbow files, unit or network resources already present: {existing}')


def relax_rp_filter(cleanup):
    interface = json.loads(run('ip', '-j', '-4', 'route', 'show', 'default'))[0]['dev']
    scopes = dict.fromkeys(('all', 'default', interface))
    values = {scope: int(Path(f'/proc/sys/net/ipv4/conf/{scope}/rp_filter').read_text())
              for scope in scopes}
    if max(values.values()) != 1:
        return
    for scope, old in values.items():
        key = f'net.ipv4.conf.{scope}.rp_filter'
        cleanup.callback(sudo, 'sysctl', '-w', f'{key}={old}')
        sudo('sysctl', '-w', f'{key}=2')
    check('strict rp_filter=1 relaxed to 2 for this VM test')


def seed_polkit_rule(user):
    # The disposable VM has no desktop password dialog; the rule stands in for it.
    sudo_tee(POLKIT_RULE, f'''// #427 CI pre-seed: replaces the desktop password dialog on this VM.
polkit.addRule(function(action, subject) {{
    if (action.id == 'org.crossbow.tun' && subject.user == {json.dumps(user)}) {{
        return polkit.Result.YES;
    }}
}});
''')
    check('polkit rule pre-seeded for the CI user in place of the desktop password dialog')


def uninstall():
    if service_load_state() != 'not-found':
        sudo('systemctl', 'stop', UNIT)
    # Keep the binaries in place if the service still needs them for recovery.
    wait(owned_resources_absent, timeout=15)
    assert not JOURNAL.exists(), 'recovery journal still present'
    sudo('rm', '-f', SERVICE, CORE, POLICY)
    if INSTALL_DIR.is_dir():
        sudo('rmdir', str(INSTALL_DIR))


def save_journal(output):
    journal = try_run('sudo', '-n', 'journalctl', '-u', UNIT, '--no-pager', '-o', 'short-iso')
    (output / 'service-journal.log').write_text(journal.stdout or journal.stderr or '')


def install(service, core):
    sudo(str(INSTALL_SCRIPT), str(service), str(core))
    for path in (SERVICE, CORE):
        meta = os.stat(path)
        assert meta.st_uid == 0, path
        assert meta.st_mode & 0o777 == 0o755 and not meta.st_mode & 0o022, path
    assert Path(POLICY).is_file()
    check('install-service.sh installed root-owned, group/other-unwritable binaries and the polkit action')


def authorize_and_verify(uid):
    result = subprocess.run(['pkexec', '--disable-internal-agent', SERVICE, '--authorize'],
                            capture_output=True, text=True, check=False, timeout=120)
    assert result.returncode == 0, f'pkexec authorize failed: {result.stderr.strip()}'
    wait(lambda: SOCKET.exists(), timeout=15)
    meta = os.stat(RUNTIME)
    assert meta.st_uid == uid and meta.st_mode & 0o777 == 0o700, 'runtime directory ownership/mode'
    unit = systemctl(['MainPID', 'User', 'LoadState', 'Transient', 'ActiveState', 'ExecStart'])
    assert int(unit['MainPID']) > 0, unit
    assert int(unit['User']) == uid, unit
    assert unit['LoadState'] == 'loaded' and unit['Transient'] == 'yes', unit
    assert unit['ActiveState'] == 'active', unit
    assert unit['ExecStart'].startswith(
        '{ path=/usr/lib/crossbow/crossbow-service ; argv[]=/usr/lib/crossbow/crossbow-service --serve ;'), unit
    check('pkexec authorized and started the transient crossbow-tun unit as the calling user')


def fetch(url):
    """Capture curl's actual socket endpoints so each probe can be verified in the core log."""
    failure = None
    for _ in range(3):
        result = try_run('curl', '--disable', '--noproxy', '*', '--ipv4', '-sS',
                         '--max-time', '25', '-o', '/dev/null', '-w',
                         '%{http_code} %{local_ip} %{local_port} %{remote_ip} %{remote_port}', url)
        fields = result.stdout.split()
        if result.returncode == 0 and len(fields) == 5 and fields[0] != '000':
            code, source, source_port, destination, destination_port = fields
            return int(code), f'{source}:{source_port}', destination, destination_port
        failure = result.stderr.strip()
        time.sleep(4)
    raise AssertionError(f'curl never reached {url}: {failure}')


def saw_direct(log, source, destinations, port):
    # Mihomo v1.19.32 may append a process name and UID to the source address.
    target = '|'.join(map(re.escape, destinations))
    return re.search(
        rf'\[TCP\] {re.escape(source)}(?:\([^\n]*?\))? --> (?:{target}):{port} '
        r'(?:match [^\n]+ )?using DIRECT(?=["\s]|$)',
        log,
    ) is not None


def verify_session_resources(stream, result, uid):
    link = next(row for row in json.loads(run('ip', '-j', 'link', 'show'))
                if row['ifname'] == DEVICE)
    assert link.get('ifindex') == result['ifindex'], (link, result)
    for family in (4, 6):
        rule = next(row for row in rules_json(family)
                    if same_number(row.get('priority'), PRIORITY))
        assert same_number(rule.get('table'), TABLE), rule
        assert same_number(rule.get('protocol'), PROTOCOL), rule
    v4 = json.loads(run('ip', '-j', '-4', 'route', 'show', 'table', TABLE))
    default = next(row for row in v4 if row['dst'] == 'default')
    assert default.get('dev') == DEVICE and same_number(default.get('protocol'), PROTOCOL), default
    assert any(row.get('type') == 'throw' for row in v4), v4
    v6 = json.loads(run('ip', '-j', '-6', 'route', 'show', 'table', TABLE))
    assert any(row['dst'] == 'default' and row.get('dev') == DEVICE for row in v6), v6
    cap = next(line.split()[1] for line in Path(f'/proc/{result["pid"]}/status').read_text().splitlines()
               if line.startswith('CapEff:'))
    assert int(cap, 16) == 1 << 12, cap
    runtime = SESSION_RUNTIME.read_text()
    assert 'device: Crossbow' in runtime and 'dns-hijack' in runtime, runtime
    status = request(stream, {'cmd': 'status'})
    assert status.get('ok') and status.get('active'), status
    assert (status['pid'], status['ifindex']) == (result['pid'], result['ifindex']), status
    check('real host gained the Crossbow device, table 20220 rules/routes (protocol 242, both families), '
          'and the core runs with CAP_NET_ADMIN only')


def verify_https_probe(core_log):
    failures = []
    for url, host in HTTPS_PROBES:
        try:
            code, source, destination, port = fetch(url)
        except AssertionError as error:
            failures.append(f'{url}: {error}')
            print(f'[probe] {failures[-1]}', flush=True)
            continue
        if code != 204:
            failures.append(f'{url}: expected 204, received {code}')
            print(f'[probe] {failures[-1]}', flush=True)
            continue
        # 成功响应仍须证明经过本次 TUN 连接。缺少日志不能靠切换站点掩盖。
        wait(lambda: saw_direct(core_log.read_text(), source, (host, destination), port), timeout=5)
        check(f'HTTPS 204 from {host} matched a DIRECT connection in the TUN core log')
        return
    raise AssertionError('no HTTPS 204 endpoint succeeded: ' + '; '.join(failures))


def verify_data_plane(core_log, original_private_route):
    code, source, destination, port = fetch('http://1.1.1.1')
    assert 200 <= code < 400, ('http://1.1.1.1', code)
    wait(lambda: saw_direct(core_log.read_text(), source, ('1.1.1.1', destination), port), timeout=5)
    verify_https_probe(core_log)
    check('public IPv4 HTTP and HTTPS probes each matched a DIRECT connection in the TUN core log')
    assert private_route() == original_private_route, 'private destination route changed'
    public = run('ip', 'route', 'get', '8.8.8.8')
    assert DEVICE in public, public
    check('private destinations keep the original route while public defaults route into table 20220')


def private_route():
    route = json.loads(run('ip', '-j', 'route', 'get', '192.168.1.1'))[0]
    return tuple(route.get(field) for field in ('dev', 'gateway', 'table', 'prefsrc'))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--disposable-vm', action='store_true', required=True,
                        help='confirm this is a disposable VM without an existing Crossbow installation')
    parser.add_argument('--service', type=Path, required=True)
    parser.add_argument('--core', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=Path('target/linux-vm-e2e'))
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    (output / 'report.json').write_text(json.dumps({'passed': False, 'mode': 'vm e2e in progress'}) + '\n')

    if os.getuid() == 0:
        parser.error('run unprivileged: pkexec must supply PKEXEC_UID')
    service, core = args.service.resolve(), args.core.resolve()
    assert service.is_file() and os.access(service, os.X_OK), service
    assert core.is_file() and os.access(core, os.X_OK), core

    require_clean_host()
    uid = os.getuid()
    baseline = network_state()
    original_private_route = private_route()
    config = output / 'config.yaml'
    mixed, controller = free_port(), free_port()
    while controller == mixed:
        controller = free_port()
    config.write_text(f'''mixed-port: {mixed}
external-controller: 127.0.0.1:{controller}
secret: {SECRET}
log-level: info
profile: {{store-selected: true}}
geox-url:
  geosite: http://127.0.0.1:1/must-not-download
  geoip: http://127.0.0.1:1/must-not-download
rules: ["MATCH,DIRECT"]
dns:
  nameserver: ["https://1.1.1.1/dns-query"]
''')
    core_log = output / 'core.log'

    def start_tun(log=core_log):
        stream = connect()
        result = request(stream, {'cmd': 'start', 'config': str(config), 'log': str(log)})
        assert result.get('ok') and result.get('active'), (
            result, log.read_text()[-4000:] if log.exists() else 'no core log')
        return stream, result

    with ExitStack() as cleanup:
        cleanup.callback(save_journal, output)
        if not Path('/dev/net/tun').exists():
            sudo('modprobe', 'tun')
        assert Path('/dev/net/tun').exists(), 'kernel TUN support missing'
        assert Path('/run/systemd/system').exists(), 'a real systemd host is required'
        run('pkexec', '--version')
        if try_run('systemctl', 'is-active', '--quiet', 'polkit').returncode != 0:
            sudo('systemctl', 'start', 'polkit')
        check('VM preconditions: /dev/net/tun, systemd, pkexec and polkit are available')
        relax_rp_filter(cleanup)
        cleanup.callback(sudo, 'rm', '-f', POLKIT_RULE)
        seed_polkit_rule(run('id', '-un').strip())
        cleanup.callback(uninstall)
        install(service, core)
        authorize_and_verify(uid)

        stream, started = start_tun()
        try:
            verify_session_resources(stream, started, uid)
            verify_data_plane(core_log, original_private_route)
        finally:
            stopped = request(stream, {'cmd': 'stop'})
        assert stopped.get('ok') and stopped.get('stopped'), stopped
        stream.close()
        wait(lambda: owned_resources_absent(), timeout=15)
        assert not JOURNAL.exists(), 'stop must consume the recovery journal'
        meta = os.stat(core_log)
        assert meta.st_uid == uid and meta.st_mode & 0o777 == 0o600 and meta.st_size > 0, meta
        unit = systemctl(['ActiveState', 'MainPID'])
        assert unit['ActiveState'] == 'active' and int(unit['MainPID']) > 0, unit
        check('stop restores all owned resources; the log persists as a 0600 user file '
              'and the authorized unit stays idle for reuse')

        service_pid = int(unit['MainPID'])
        stream, _ = start_tun(output / 'restart-core.log')
        assert int(systemctl(['MainPID'])['MainPID']) == service_pid, 'service PID changed on restart'
        assert request(stream, {'cmd': 'stop'}).get('stopped')
        stream.close()
        wait(lambda: owned_resources_absent(), timeout=15)
        check('configuration restart reuses the already-authorized idle service unit')

        stream, _ = start_tun(output / 'crash-core.log')
        assert int(systemctl(['MainPID'])['MainPID']) == service_pid
        os.kill(service_pid, signal.SIGKILL)
        stream.close()
        def recovered():
            return systemctl(['ActiveState'])['ActiveState'] in ('inactive', 'failed') \
                and owned_resources_absent()
        wait(recovered, timeout=45)
        assert not JOURNAL.exists(), 'ExecStopPost did not consume the recovery journal'
        check('SIGKILLed service is recovered by the real systemd ExecStopPost cleanup on the host')

    for path in (SERVICE, CORE, POLICY, POLKIT_RULE):
        assert not os.path.lexists(path), path
    assert try_run('pkexec', '--disable-internal-agent', SERVICE, '--authorize').returncode != 0, (
        'pkexec authorization must fail without the installed service')
    check('uninstall removes the test binaries and polkit files; pkexec no longer succeeds')

    assert network_state() == baseline, 'host routes/rules/DNS changed by the e2e'
    check('host routes/rules/DNS are identical before and after the run')

    result = {'passed': True, 'checks': CHECKS, 'systemd_authorization_tested': True,
              'mode': 'real systemd + polkit + pkexec VM acceptance'}
    (output / 'report.json').write_text(json.dumps(result, indent=2, ensure_ascii=False) + '\n')
    print(json.dumps(result, ensure_ascii=False))


if __name__ == '__main__':
    main()
