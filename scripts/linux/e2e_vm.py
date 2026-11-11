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
    python3 scripts/linux/e2e_vm.py \
        --service target/release/crossbow-service \
        --core apps/desktop/src-tauri/binaries/mihomo-x86_64-unknown-linux-gnu \
        --output target/linux-vm-e2e
"""
import argparse
import http.client
import json
import os
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

CHECKS = []


def check(message):
    CHECKS.append(message)
    print(f'[ok] {message}', flush=True)


def sudo(*args):
    return run('sudo', *args)


def try_run(*args):
    return subprocess.run(args, capture_output=True, text=True, check=False)


def sudo_tee(path, text):
    subprocess.run(['sudo', 'tee', str(path)], input=text, text=True,
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


def relax_rp_filter():
    interface = json.loads(run('ip', '-j', '-4', 'route', 'show', 'default'))[0]['dev']

    def effective():
        return max(int(Path(f'/proc/sys/net/ipv4/conf/{path}/rp_filter').read_text())
                   for path in ('all', 'default', interface))

    value = effective()
    if value == 1:
        for scope in ('all', 'default', interface):
            sudo('sysctl', '-w', f'net.ipv4.conf.{scope}.rp_filter=2')
        assert effective() == 2
        check('strict rp_filter=1 relaxed to 2 as a documented disposable-VM precondition')
    else:
        check(f'rp_filter effective value {value} already permits takeover')


def seed_polkit_rule(user):
    # The disposable VM has no desktop password dialog; the rule stands in for it.
    sudo_tee(Path('/etc/polkit-1/rules.d/49-crossbow-vm-e2e.rules'), f'''// #427 CI pre-seed: replaces the desktop password dialog on this VM.
polkit.addRule(function(action, subject) {{
    if (action.id == 'org.crossbow.tun' && subject.user == '{user}') {{
        return polkit.Result.YES;
    }}
}});
''')
    check('polkit rule pre-seeded for the CI user in place of the desktop password dialog')


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


def controller_api(controller, path):
    connection = http.client.HTTPConnection('127.0.0.1', controller, timeout=5)
    try:
        connection.request('GET', path, None, {'Authorization': f'Bearer {SECRET}'})
        response = connection.getresponse()
        body = response.read()
        assert response.status == 200, (response.status, body)
        return json.loads(body) if body else None
    finally:
        connection.close()


def fetch(url):
    """Public fetch through the TUN; retry rides out first-use DoH setup."""
    failure = None
    for _ in range(3):
        result = try_run('curl', '-sS', '--max-time', '25', '-o', '/dev/null',
                         '-w', '%{http_code}', url)
        code = result.stdout.strip()
        if code and code != '000':
            return code
        failure = result.stderr.strip()
        time.sleep(4)
    raise AssertionError(f'curl never reached {url}: {failure}')


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


def verify_data_plane(controller):
    before = controller_api(controller, '/connections') or {}
    assert fetch('http://1.1.1.1') != '000'
    assert fetch('https://api.github.com/zen') == '200'
    after = controller_api(controller, '/connections') or {}
    connections = after.get('connections') or []
    assert connections, 'mihomo saw no connections: traffic did not traverse TUN'
    assert any('DIRECT' in chain for connection in connections
               for chain in connection.get('chains', [])), json.dumps(connections)[:2000]
    assert after.get('downloadTotal', 0) - before.get('downloadTotal', 0) > 1024, 'byte counters did not move'
    check('public IPv4 HTTP and HTTPS traversed the real TUN device through mihomo DIRECT '
          '(connections API and byte counters confirm)')
    private = run('ip', 'route', 'get', '192.168.1.1')
    assert DEVICE not in private and TABLE not in private, private
    public = run('ip', 'route', 'get', '8.8.8.8')
    assert DEVICE in public, public
    check('private destinations keep the original route while public defaults route into table 20220')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--service', type=Path, required=True)
    parser.add_argument('--core', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=Path('target/linux-vm-e2e'))
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    (output / 'report.json').write_text(json.dumps({'passed': False, 'mode': 'vm e2e in progress'}) + '\n')

    assert os.getuid() != 0, 'run unprivileged: pkexec must supply PKEXEC_UID'
    service, core = args.service.resolve(), args.core.resolve()
    assert service.is_file() and os.access(service, os.X_OK), service
    assert core.is_file() and os.access(core, os.X_OK), core

    uid = int(run('id', '-u').strip())
    baseline = network_state()
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
rules: [MATCH,DIRECT]
dns:
  nameserver: [https://1.1.1.1/dns-query]
''')
    core_log = output / 'core.log'

    def start_tun():
        stream = connect()
        result = request(stream, {'cmd': 'start', 'config': str(config), 'log': str(core_log)})
        assert result.get('ok') and result.get('active'), (
            result, core_log.read_text()[-4000:] if core_log.exists() else 'no core log')
        return stream, result

    try:
        if not Path('/dev/net/tun').exists():
            sudo('modprobe', 'tun')
        assert Path('/dev/net/tun').exists(), 'kernel TUN support missing'
        assert Path('/run/systemd/system').exists(), 'a real systemd host is required'
        run('pkexec', '--version')
        if try_run('systemctl', 'is-active', '--quiet', 'polkit').returncode != 0:
            sudo('systemctl', 'start', 'polkit')
        check('VM preconditions: /dev/net/tun, systemd, pkexec and polkit are available')
        relax_rp_filter()
        seed_polkit_rule(run('id', '-un').strip())
        install(service, core)
        authorize_and_verify(uid)

        stream, started = start_tun()
        try:
            verify_session_resources(stream, started, uid)
            verify_data_plane(controller)
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

        stream, _ = start_tun()
        assert request(stream, {'cmd': 'stop'}).get('stopped')
        stream.close()
        wait(lambda: owned_resources_absent(), timeout=15)
        check('configuration restart reuses the already-authorized idle service unit')

        stream, started = start_tun()
        service_pid = int(systemctl(['MainPID'])['MainPID'])
        os.kill(service_pid, signal.SIGKILL)
        stream.close()
        def recovered():
            return systemctl(['ActiveState'])['ActiveState'] in ('inactive', 'failed') \
                and owned_resources_absent()
        wait(recovered, timeout=45)
        assert not JOURNAL.exists(), 'ExecStopPost did not consume the recovery journal'
        check('SIGKILLed service is recovered by the real systemd ExecStopPost cleanup on the host')

        wait(lambda: try_run('systemctl', 'is-active', '--quiet', UNIT).returncode != 0, timeout=30)
        for path in (SERVICE, CORE, POLICY):
            sudo('rm', '-f', path)
            assert not Path(path).exists(), path
        assert try_run('pkexec', '--disable-internal-agent', SERVICE, '--authorize').returncode != 0, \
            'pkexec authorization must fail without the installed service'
        check('uninstall removes the binaries and policy; pkexec authorization no longer succeeds')

        assert network_state() == baseline, 'host routes/rules/DNS changed by the e2e'
        check('host routes/rules/DNS are identical before and after the run')
    finally:
        journal = try_run('sudo', 'journalctl', '-u', UNIT, '--no-pager', '-o', 'short-iso')
        (output / 'service-journal.log').write_text(journal.stdout or journal.stderr or '')

    result = {'passed': True, 'checks': CHECKS, 'systemd_authorization_tested': True,
              'mode': 'real systemd + polkit + pkexec VM acceptance'}
    (output / 'report.json').write_text(json.dumps(result, indent=2, ensure_ascii=False) + '\n')
    print(json.dumps(result, ensure_ascii=False))


if __name__ == '__main__':
    main()
