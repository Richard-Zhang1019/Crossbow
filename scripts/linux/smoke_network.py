#!/usr/bin/env python3
"""Exercise the real Linux broker/core inside private user, PID, mount and network namespaces.
No sudo, service installation, host routes, DNS or firewall writes. Requires bubblewrap, iproute2, util-linux and wireguard-tools.
"""
import argparse
import errno
import hashlib
import http.client
import json
import os
from pathlib import Path
import shutil
import shlex
import signal
import socket
import socketserver
import struct
import subprocess
import threading
import time

ROOT = Path(__file__).resolve().parents[2]
SERVICE = '/usr/lib/crossbow/crossbow-service'
SOCKET = '/run/crossbow-tun/control.sock'


def run(*args):
    result = subprocess.run(args, capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError(f'{args[0]} failed: {result.stderr.strip()}')
    return result.stdout


def network_state():
    result = []
    for family in ('-4', '-6'):
        routes = json.loads(run('ip', '-j', family, 'route', 'show', 'table', 'all'))
        rules = json.loads(run('ip', '-j', family, 'rule', 'show'))
        result.extend([sorted(routes, key=lambda row: json.dumps(row, sort_keys=True)), rules])
    result.append(Path('/etc/resolv.conf').read_text())
    return result


def snapshot():
    return hashlib.sha256(json.dumps(network_state(), sort_keys=True).encode()).hexdigest()


def wait(check, timeout=12):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = check()
        if value:
            return value
        time.sleep(.1)
    raise AssertionError('condition did not become true')


def request(stream, value):
    stream.sendall(json.dumps(value).encode() + b'\n')
    data = bytearray()
    while not data.endswith(b'\n'):
        part = stream.recv(1)
        if not part:
            raise AssertionError('service disconnected: ' + data.decode())
        data.extend(part)
        assert len(data) <= 4096
    return json.loads(data)


def connect():
    stream = socket.socket(socket.AF_UNIX)
    stream.settimeout(35)
    stream.connect(SOCKET)
    return stream


def free_port():
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        return listener.getsockname()[1]


WG_DESTINATIONS = ('198.51.100.12', '2001:db8:4::12')
WG_SOURCES = ('10.67.0.2/32', 'fd24::2/128')


def wireguard_peer(output):
    listeners = []
    for address in WG_DESTINATIONS:
        family = socket.AF_INET6 if ':' in address else socket.AF_INET
        tcp = socket.socket(family, socket.SOCK_STREAM)
        tcp.bind((address, 18080))
        tcp.listen()
        udp = socket.socket(family, socket.SOCK_DGRAM)
        udp.bind((address, 53))
        listeners.extend([tcp, udp])

    def handle(listener):
        while True:
            if listener.type == socket.SOCK_STREAM:
                with listener.accept()[0] as stream:
                    stream.settimeout(3)
                    assert stream.recv(1024) == b'wireguard-fixture'
                    stream.sendall(b'wireguard-peer')
            else:
                query, sender = listener.recvfrom(4096)
                header = query[:2] + struct.pack('!5H', 0x8180, 1, 1, 0, 0)
                answer = b'\xc0\x0c' + struct.pack('!2HIH4B', 1, 1, 60, 4, 10, 67, 0, 5)
                listener.sendto(header + query[12:] + answer, sender)

    for listener in listeners:
        threading.Thread(target=handle, args=(listener,), daemon=True).start()
    (output / 'wireguard-peer-ready').touch()
    threading.Event().wait()


def wireguard_fixture(output, processes):
    namespace = subprocess.Popen(['unshare', '--net', '--', 'sleep', '180'])
    processes.append(namespace)
    wait(lambda: namespace.poll() is None
         and os.readlink(f'/proc/{namespace.pid}/ns/net') != os.readlink('/proc/self/ns/net'))
    peer = ['nsenter', '--target', str(namespace.pid), '--net', '--']
    run('ip', 'link', 'add', 'wg-underlay', 'type', 'veth', 'peer', 'name', 'wg-remote')
    run('ip', 'link', 'set', 'wg-remote', 'netns', str(namespace.pid))
    run('ip', 'link', 'set', 'dev', 'wg-underlay', 'addrgenmode', 'none')
    run('ip', 'addr', 'add', '203.0.113.129/30', 'dev', 'wg-underlay')
    run('ip', 'link', 'set', 'wg-underlay', 'up')
    run(*peer, 'ip', 'addr', 'add', '203.0.113.130/30', 'dev', 'wg-remote')
    run(*peer, 'ip', 'link', 'set', 'wg-remote', 'up')
    run('ip', 'link', 'add', 'wg-test', 'type', 'wireguard')
    for device in ('wg-underlay', 'wg-test'):
        Path(f'/proc/sys/net/ipv4/conf/{device}/rp_filter').write_text('2')
    run(*peer, 'ip', 'link', 'add', 'wg-peer', 'type', 'wireguard')
    keys = [run('wg', 'genkey') for _ in range(2)]
    public = [subprocess.run(['wg', 'pubkey'], input=key, text=True,
                             check=True, capture_output=True).stdout.strip() for key in keys]
    destinations = [f'{ip}/{128 if ":" in ip else 32}' for ip in WG_DESTINATIONS]
    subprocess.run(['wg', 'set', 'wg-test', 'private-key', '/dev/stdin',
                    'peer', public[1], 'allowed-ips', ','.join(destinations),
                    'endpoint', '203.0.113.130:51822'], input=keys[0], text=True, check=True)
    subprocess.run([*peer, 'wg', 'set', 'wg-peer', 'listen-port', '51822',
                    'private-key', '/dev/stdin', 'peer', public[0],
                    'allowed-ips', ','.join(WG_SOURCES)], input=keys[1], text=True, check=True)
    run('ip', 'link', 'set', 'wg-test', 'up')
    run(*peer, 'ip', 'link', 'set', 'wg-peer', 'up')
    for family, src, dest in zip(['-4', '-6'], WG_SOURCES, destinations):
        dad = ['nodad'] if family == '-6' else []
        run('ip', family, 'addr', 'add', src, 'dev', 'wg-test', *dad)
        run(*peer, 'ip', family, 'addr', 'add', dest, 'dev', 'wg-peer', *dad)
        run('ip', family, 'route', 'add', dest, 'dev', 'wg-test')
        run(*peer, 'ip', family, 'route', 'add', src, 'dev', 'wg-peer')
    ready = output / 'wireguard-peer-ready'
    ready.unlink(missing_ok=True)
    process = subprocess.Popen([*peer, '/usr/bin/python3', str(Path(__file__).resolve()),
                                '--wireguard-peer', '--output', str(output)])
    processes.append(process)
    wait(lambda: process.poll() is None and ready.exists())


def check_wireguard_data():
    query = struct.pack('!6H', 207, 0x100, 1, 0, 0, 0) + b'\x0cwireguard-wg\x04test\0' + struct.pack('!2H', 1, 1)
    for address in WG_DESTINATIONS:
        family = socket.AF_INET6 if ':' in address else socket.AF_INET
        with socket.socket(family, socket.SOCK_STREAM) as tcp:
            tcp.settimeout(5)
            tcp.connect((address, 18080))
            tcp.sendall(b'wireguard-fixture')
            assert tcp.recv(1024) == b'wireguard-peer'
        with socket.socket(family, socket.SOCK_DGRAM) as dns:
            dns.settimeout(5)
            dns.sendto(query, (address, 53))
            answer, sender = dns.recvfrom(4096)
            assert sender[0] == address and answer[:2] == query[:2] and answer.endswith(b'\x0a\x43\x00\x05')


def inside(output):
    # This is a test runner guard, not a production mode of the broker.
    assert os.environ.get('CROSSBOW_TEST_NETNS') == os.readlink('/proc/self/ns/net')
    assert os.getuid() == 0 and os.environ['CROSSBOW_HOST_NETNS'] != os.readlink('/proc/self/ns/net')
    os.umask(0o077)
    Path('/run/crossbow-tun').mkdir(mode=0o700)
    run('ip', 'link', 'set', 'lo', 'up')
    run('ip', 'link', 'add', 'uplink', 'type', 'dummy')
    run('ip', 'addr', 'add', '192.0.2.2/24', 'dev', 'uplink')
    run('ip', '-6', 'addr', 'add', '2001:db8:1::2/64', 'dev', 'uplink', 'nodad')
    run('ip', 'link', 'set', 'uplink', 'up')
    run('ip', 'route', 'add', 'default', 'dev', 'uplink')
    run('ip', '-6', 'route', 'add', 'default', 'dev', 'uplink')
    # These sysctls belong to this asserted-private network namespace only.
    Path('/proc/sys/net/ipv4/conf/all/rp_filter').write_text('0')
    rp_filter = Path('/proc/sys/net/ipv4/conf/uplink/rp_filter')
    rp_filter.write_text('1')
    baseline = network_state()
    checks = []
    proxies = []

    class Proxy(socketserver.StreamRequestHandler):
        def handle(self):
            first = self.rfile.readline().decode().strip()
            while self.rfile.readline().strip():
                pass
            assert first.startswith('CONNECT '), first
            proxies.append(first)
            self.wfile.write(b'HTTP/1.1 200 Connection established\r\n\r\n')
            self.wfile.flush()
            assert self.rfile.readline().startswith(b'GET ')
            while self.rfile.readline().strip():
                pass
            body = b'crossbow-isolated-tun-ok'
            self.wfile.write(b'HTTP/1.1 200 OK\r\nContent-Length: ' + str(len(body)).encode()
                             + b'\r\nConnection: close\r\n\r\n' + body)
            self.wfile.flush()

    class Server(socketserver.ThreadingTCPServer):
        daemon_threads = True
        allow_reuse_address = True

    server = Server(('127.0.0.1', 0), Proxy)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    config = output / 'config.yaml'
    core_log = output / 'core.log'
    # Minimal GeoSiteList: TEST plus CN, which mihomo probes during validation.
    # Keeping the only GeoSite file lowercase exercises Linux's actual filename.
    domain = b'\x08\x03\x12\x0f' + b'fixture.invalid'
    geosite = b''
    for name in [b'CN', b'TEST']:
        site = b'\x0a' + bytes([len(name)]) + name + b'\x12' + bytes([len(domain)]) + domain
        geosite += b'\x0a' + bytes([len(site)]) + site
    (output / 'geosite.dat').write_bytes(geosite)
    (output / 'cache.db').unlink(missing_ok=True)
    mixed, controller = free_port(), free_port()
    while controller == mixed:
        controller = free_port()
    config.write_text(f'''mixed-port: {mixed}
external-controller: 127.0.0.1:{controller}
secret: crossbow-isolated-test-secret
log-level: debug
profile: {{store-selected: true}}
geox-url:
  geosite: http://127.0.0.1:1/must-not-download
proxies:
  - name: fixture
    type: http
    server: 127.0.0.1
    port: {server.server_address[1]}
proxy-groups:
  - name: selection
    type: select
    proxies: [fixture, DIRECT]
rules: ["GEOSITE,TEST,selection", "MATCH,selection"]
dns:
  nameserver: [127.0.0.1:65530]
''')
    processes = []

    def api(method, path, value=None):
        connection = http.client.HTTPConnection('127.0.0.1', controller, timeout=3)
        try:
            connection.request(method, path, None if value is None else json.dumps(value),
                               {'Authorization': 'Bearer crossbow-isolated-test-secret',
                                'Content-Type': 'application/json'})
            response = connection.getresponse()
            body = response.read()
            assert response.status in [200, 204], (response.status, body)
            return json.loads(body) if body else None
        finally:
            connection.close()

    def select(name):
        previous = (output / 'cache.db').stat().st_mtime_ns
        api('PUT', '/proxies/selection', {'name': name})
        wait(lambda: (output / 'cache.db').stat().st_mtime_ns != previous)
        assert api('GET', '/proxies/selection')['now'] == name

    def ordinary():
        log = (output / 'ordinary.log').open('wb')
        process = subprocess.Popen(['/usr/lib/crossbow/mihomo', '-d', str(output), '-f', str(config)],
                                   stdout=log, stderr=log)
        log.close()
        processes.append(process)
        def ready():
            assert process.poll() is None, (output / 'ordinary.log').read_text()
            try: return api('GET', '/proxies/selection')
            except (OSError, http.client.HTTPException): return False
        wait(ready)
        return process

    def launch():
        log = (output / f'service-{len(processes)}.log').open('wb')
        # Match the production service capability bound; no root filesystem powers.
        command = ['setpriv', '--bounding-set=-all,+net_admin', '--inh-caps=-all,+net_admin',
                   '--ambient-caps=-all,+net_admin', '--no-new-privs', SERVICE, '--serve']
        socket_path = Path(SOCKET)
        previous = socket_path.stat().st_ino if socket_path.exists() else None
        process = subprocess.Popen(command, stdout=log, stderr=log)
        log.close()
        processes.append(process)

        def socket_ready():
            try:
                return socket_path.stat().st_ino != previous
            except FileNotFoundError:
                return False

        wait(lambda: socket_ready() or process.poll() is not None)
        assert process.poll() is None, f'service failed: {list(output.glob("service-*.log"))[-1].read_text()}'
        return process

    def start(wireguard=False):
        stream = connect()
        result = request(stream, {'cmd': 'start', 'config': str(config), 'log': str(core_log),
                                  **({'wireguard_coexist': True} if wireguard else {})})
        assert result.get('ok') and result.get('active'), (result, core_log.read_text()[-5000:] if core_log.exists() else 'no core log')
        return stream, result

    def clean():
        assert not any(link['ifname'] == 'Crossbow' for link in json.loads(run('ip', '-j', 'link')))
        current = network_state()
        if current != baseline:
            (output / 'namespace-network-diff.json').write_text(
                json.dumps({'before': baseline[:-1], 'after': current[:-1]}, indent=2))
            raise AssertionError('namespace routes/rules/DNS were not restored')
        assert not Path('/run/crossbow-tun/network.json').exists()

    def check_public_data():
        for address in ['203.0.113.77', '2001:db8:2::77']:
            family = socket.AF_INET6 if ':' in address else socket.AF_INET
            with socket.socket(family) as connection:
                connection.settimeout(8)
                connection.connect((address, 80))
                connection.sendall(b'GET / HTTP/1.1\r\nHost: fixture.invalid\r\nConnection: close\r\n\r\n')
                response = bytearray()
                while part := connection.recv(4096):
                    response.extend(part)
                assert response.endswith(b'crossbow-isolated-tun-ok'), response

    try:
        process = launch()
        with connect() as rejected:
            result = request(rejected, {'cmd': 'start', 'config': str(config), 'log': str(core_log)})
            assert not result['ok'] and result['stopped'] and 'rp_filter=1' in result['error'], result
        assert rp_filter.read_text().strip() == '1'
        clean()
        checks.append('strict rp_filter is rejected without changing routes or the sysctl')
        rp_filter.write_text('2')
        explicit = ordinary()
        select('DIRECT')
        explicit.terminate(); explicit.wait(timeout=5)
        cache_inode = (output / 'cache.db').stat().st_ino
        stream, state = start()
        assert Path('/run/crossbow-tun/session/geosite.dat').read_bytes() == geosite
        checks.append('lowercase geosite.dat serves GEOSITE rules without a download')
        assert api('GET', '/proxies/selection')['now'] == 'DIRECT'
        assert Path('/run/crossbow-tun/session/cache.db').stat().st_ino == cache_inode
        checks.append('ordinary proxy selection is preserved when entering TUN')
        select('fixture')
        status_text = Path(f'/proc/{state["pid"]}/status').read_text()
        cap = next(line.split()[1] for line in status_text.splitlines() if line.startswith('CapEff:'))
        assert int(cap, 16) == 1 << 12, cap
        checks.append('real core runs with CAP_NET_ADMIN only')
        check_public_data()
        assert len(proxies) == 2
        checks.append('unproxied IPv4 and IPv6 TCP requests traverse TUN and real mihomo')
        query = struct.pack('!6H', 99, 0x100, 1, 0, 0, 0) + b'\x0dlinux-fixture\x04test\0' + struct.pack('!2H', 1, 1)
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as dns:
            dns.settimeout(5)
            dns.sendto(query, ('1.1.1.1', 53))
            answer, _ = dns.recvfrom(4096)
            assert answer[:2] == query[:2] and struct.unpack('!H', answer[6:8])[0] > 0
        assert not (output / 'resolver-called').exists(), 'core tried to invoke a system DNS helper'
        checks.append('UDP DNS is handled in TUN without changing resolver configuration or invoking resolvectl')
        select('DIRECT')
        assert request(stream, {'cmd': 'stop'})['stopped']
        stream.close()
        clean()
        checks.append('normal stop restores the namespace network exactly')
        stream, state = start()
        assert api('GET', '/proxies/selection')['now'] == 'DIRECT'
        assert request(stream, {'cmd': 'stop'})['stopped']
        stream.close()
        clean()
        checks.append('configuration restart reuses the authorized idle service')
        assert (output / 'cache.db').stat().st_ino == cache_inode
        checks.append('TUN restart preserves the selected node and original cache database')
        # Occupied listeners must not be killed or mistaken for readiness.
        with socket.socket() as occupied:
            occupied.bind(('127.0.0.1', mixed)); occupied.listen()
            with connect() as refused:
                result = request(refused, {'cmd': 'start', 'config': str(config), 'log': str(core_log)})
                assert not result['ok'] and result['stopped'], result
            clean()
        checks.append('occupied mixed port is rejected without modifying the network')
        stream, state = start()
        stream.close()  # Same kernel EOF as a killed desktop process.
        process.wait(timeout=10)
        clean()
        checks.append('desktop disconnect stops its core and releases all recorded resources')
        process = launch()
        stream, state = start()
        select('fixture')
        os.kill(state['pid'], signal.SIGKILL)
        process.wait(timeout=10); stream.close()
        clean()
        checks.append('core SIGKILL is detected and its network resources are released')
        explicit = ordinary()
        wait(lambda: api('GET', '/proxies/selection')['now'] == 'fixture')
        assert (output / 'cache.db').stat().st_ino == cache_inode
        explicit.terminate(); explicit.wait(timeout=5)
        checks.append('TUN selection survives core SIGKILL and return to ordinary proxy mode')
        process = launch()
        stream, state = start()
        run('ip', 'rule', 'add', 'priority', '11000', 'table', 'main')
        process.wait(timeout=10); stream.close()
        assert any(r['priority'] == 11000 for r in json.loads(run('ip', '-j', 'rule')))
        run('ip', 'rule', 'del', 'priority', '11000', 'table', 'main')
        clean()
        checks.append('new foreign policy route stops Crossbow while preserving the foreign rule')
        process = launch()
        stream, state = start()
        run('ip', 'rule', 'del', 'not', 'fwmark', '49451', 'table', '20220', 'priority', '12020', 'protocol', '242')
        process.wait(timeout=10); stream.close()
        clean()
        checks.append('missing owned rule is detected; remaining owned resources are cleaned')
        process = launch()
        stream, state = start()
        os.kill(state['pid'], signal.SIGKILL)
        process.kill(); process.wait(timeout=5); stream.close()
        # Exercise the same executable as ExecStopPost. systemd itself is not
        # running inside this namespace, so this is not a systemd acceptance claim.
        run(SERVICE, '--cleanup')
        clean()
        checks.append('standalone ExecStopPost cleanup recovers a killed service/core pair')
        wireguard_fixture(output, processes)
        baseline = network_state()

        def refused(wireguard, message):
            with connect() as client:
                result = request(client, {'cmd': 'start', 'config': str(config), 'log': str(core_log),
                                          'wireguard_coexist': wireguard})
                assert not result['ok'] and result['stopped'] and message in result['error'], result

        process = launch()
        refused(False, 'wg-test')
        clean()
        checks.append('WireGuard continues to block TUN unless coexistence is explicitly enabled')
        run('ip', '-4', 'route', 'add', 'default', 'dev', 'wg-test', 'metric', '700')
        refused(True, 'WireGuard')
        run('ip', '-4', 'route', 'del', 'default', 'dev', 'wg-test', 'metric', '700')
        clean()
        checks.append('coexistence rejects a WireGuard default route')
        stream, state = start(wireguard=True)
        assert api('GET', '/proxies/selection')['now'] == 'fixture'
        assert 'dns-hijack: []' in Path('/run/crossbow-tun/session/runtime.yaml').read_text()
        check_public_data()
        check_wireguard_data()
        checks.append('real dual-stack WireGuard TCP and private DNS coexist with dual-stack mihomo TUN')
        assert request(stream, {'cmd': 'stop'})['stopped']
        stream.close()
        clean()
        check_wireguard_data()
        checks.append('normal coexistence stop restores the original WireGuard network exactly')

        stream, state = start(wireguard=True)
        run('ip', '-4', 'route', 'del', f'{WG_DESTINATIONS[0]}/32', 'dev', 'wg-test')
        process.wait(timeout=12)
        stream.close()
        run('ip', '-4', 'route', 'add', f'{WG_DESTINATIONS[0]}/32', 'dev', 'wg-test')
        clean()
        checks.append('withdrawal of an original WireGuard route stops the coexistence session')

        process = launch()
        stream, state = start(wireguard=True)
        # The shape is identical to an existing Crossbow route except for its table.
        route = ['throw', '192.168.0.0/16', 'table', '20221', 'proto', '242']
        run('ip', '-4', 'route', 'add', *route)
        process.wait(timeout=12)
        stream.close()
        assert any(row['dst'] == '192.168.0.0/16'
                   for row in json.loads(run('ip', '-j', '-4', 'route', 'show', 'table', '20221')))
        run('ip', '-4', 'route', 'del', *route)
        clean()
        checks.append('a foreign route copying Crossbow attributes in the wrong table forces cleanup and is preserved')

        process = launch()
        stream, state = start(wireguard=True)
        os.kill(process.pid, signal.SIGSTOP)
        try:
            run('ip', 'link', 'del', 'wg-test')
            for address in WG_DESTINATIONS:
                family = socket.AF_INET6 if ':' in address else socket.AF_INET
                with socket.socket(family, socket.SOCK_STREAM) as probe:
                    probe.settimeout(3)
                    try:
                        probe.connect((address, 18080))
                    except OSError as error:
                        assert error.errno in [errno.EHOSTUNREACH, errno.ENETUNREACH], error
                    else:
                        raise AssertionError('a protected destination reached another route')
        finally:
            os.kill(process.pid, signal.SIGCONT)
        process.wait(timeout=12)
        stream.close()
        assert not Path('/run/crossbow-tun/network.json').exists()
        for family in ['-4', '-6']:
            assert not any(str(row.get('table')) in ['20220', '20221']
                           for row in json.loads(run('ip', '-j', family, 'route', 'show', 'table', 'all')))
            assert not any(row.get('priority') in [12019, 12020]
                           for row in json.loads(run('ip', '-j', family, 'rule', 'show')))
        checks.append('protected IPv4 and IPv6 fail before the service can observe link loss; cleanup then releases owned rules')

    finally:
        for process in processes:
            if process.poll() is None:
                process.terminate()
                try: process.wait(timeout=8)
                except subprocess.TimeoutExpired: process.kill(); process.wait()
        server.shutdown()
    return {'passed': True, 'checks': checks, 'systemd_authorization_tested': False,
            'mode': 'real broker and pinned mihomo in private user/PID/mount/network namespaces'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--service', type=Path, default=ROOT / 'target/debug/crossbow-service')
    parser.add_argument('--core', type=Path, default=ROOT / 'apps/desktop/src-tauri/binaries/mihomo-x86_64-unknown-linux-gnu')
    parser.add_argument('--output', type=Path, default=ROOT / 'target/linux-validation/network')
    parser.add_argument('--inside', action='store_true', help=argparse.SUPPRESS)
    parser.add_argument('--wireguard-peer', action='store_true', help=argparse.SUPPRESS)
    args = parser.parse_args()
    output = args.output.resolve(); output.mkdir(parents=True, exist_ok=True)
    if args.wireguard_peer:
        wireguard_peer(output)
        return
    if args.inside:
        result = inside(output)
    else:
        before = snapshot()
        (output / 'report.json').write_text(json.dumps({'passed': False, 'mode': 'namespace test in progress'}) + '\n')
        fixture = output / 'install/usr/lib/crossbow'; fixture.mkdir(parents=True, exist_ok=True)
        for source, name in [(args.service, 'crossbow-service'), (args.core, 'mihomo')]:
            shutil.copyfile(source.resolve(), fixture / name)
            (fixture / name).chmod(0o755)
        helper = output / 'install/usr/bin/resolvectl'
        helper.parent.mkdir(parents=True, exist_ok=True)
        helper.write_text('#!/bin/sh\n: > ' + shlex.quote(str(output / 'resolver-called')) + '\nexit 1\n')
        helper.chmod(0o755)
        (output / 'resolver-called').unlink(missing_ok=True)
        host_ns = os.readlink('/proc/self/ns/net')
        command = ['bwrap', '--unshare-user', '--uid', '0', '--gid', '0', '--unshare-net', '--unshare-pid',
                   '--cap-add', 'ALL', '--die-with-parent', '--ro-bind', '/', '/', '--proc', '/proc',
                   '--dev', '/dev', '--dev-bind', '/dev/net/tun', '/dev/net/tun',
                   '--tmpfs', '/run', '--tmpfs', '/tmp', '--overlay-src', '/usr',
                   '--overlay-src', str(output / 'install/usr'), '--ro-overlay', '/usr',
                   '--bind', str(output), str(output), '--setenv', 'CROSSBOW_HOST_NETNS', host_ns,
                   '--', '/bin/sh', '-c', 'export CROSSBOW_TEST_NETNS=$(readlink /proc/self/ns/net); exec "$@"', 'crossbow-test',
                   '/usr/bin/python3', str(Path(__file__).resolve()), '--inside', '--output', str(output)]
        process = subprocess.run(command)
        unchanged = snapshot() == before
        if process.returncode or not unchanged:
            (output / 'report.json').write_text(json.dumps({
                'passed': False, 'exit_code': process.returncode,
                'host_network_unchanged': unchanged}) + '\n')
            raise SystemExit(f'namespace test failed ({process.returncode}); host network unchanged={unchanged}')
        result = json.loads((output / 'report.json').read_text())
        result['host_network_unchanged'] = unchanged
        result['service_sha256'] = hashlib.sha256(args.service.read_bytes()).hexdigest()
    (output / 'report.json').write_text(json.dumps(result, indent=2, ensure_ascii=False) + '\n')
    if not args.inside:
        print(json.dumps(result, ensure_ascii=False))


if __name__ == '__main__':
    main()
