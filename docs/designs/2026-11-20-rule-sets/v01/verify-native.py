#!/usr/bin/env python3
"""Run exported production renders on loopback ports; no TUN/system proxy.
First export fixtures with the ignored core native_engines_check_cached test.
"""
import argparse
import contextlib
import http.server
import json
import pathlib
import socket
import subprocess
import tempfile
import threading
import time
import urllib.request


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def verify(binary, engine, fixture):
    reached, release = threading.Event(), threading.Event()
    class Target(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            reached.set()
            release.wait(10)
            self.send_response(200)
            self.send_header('Content-Length', '2')
            self.end_headers()
            with contextlib.suppress(BrokenPipeError):
                self.wfile.write(b'ok')
        def log_message(self, *_):
            pass
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Target)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    proxy, api = free_port(), free_port()
    doc = json.loads(fixture.read_text())
    if engine == 'mihomo':
        doc.update({'mixed-port': proxy, 'bind-address': '127.0.0.1', 'allow-lan': False,
                    'external-controller': f'127.0.0.1:{api}', 'secret': 'fixture',
                    'hosts': {'chat.ai.example': '127.0.0.1'}, 'dns': {'enable': False}, 'log-level': 'silent'})
    else:
        doc['inbounds'] = [{'type': 'mixed', 'tag': 'mixed-in', 'listen': '127.0.0.1', 'listen_port': proxy}]
        doc['experimental']['clash_api'] = {'external_controller': f'127.0.0.1:{api}', 'secret': 'fixture'}
        doc['dns'] = {'servers': [{'type': 'hosts', 'tag': 'local-dns', 'predefined': {'chat.ai.example': ['127.0.0.1']}}], 'final': 'local-dns'}
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    def api_get():
        request = urllib.request.Request(f'http://127.0.0.1:{api}/connections', headers={'Authorization': 'Bearer fixture'})
        with opener.open(request, timeout=1) as response:
            return json.load(response)
    with tempfile.TemporaryDirectory(prefix='crossbow-rule-native-') as temporary:
        root = pathlib.Path(temporary)
        config = root / 'config.json'
        config.write_text(json.dumps(doc))
        args = [str(binary), '-d', str(root), '-f', str(config)] if engine == 'mihomo' else [str(binary), 'run', '-D', str(root), '-c', str(config)]
        client = None
        with (root / 'kernel.log').open('w+') as log:
            child = subprocess.Popen(args, stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + 10
                while True:
                    try:
                        api_get()
                        break
                    except (OSError, ValueError):
                        if time.monotonic() >= deadline or child.poll() is not None:
                            log.seek(0)
                            raise AssertionError('Kernel did not start: ' + log.read())
                        time.sleep(.05)
                client = socket.create_connection(('127.0.0.1', proxy), timeout=3)
                port = server.server_address[1]
                client.sendall(f'GET http://chat.ai.example:{port}/ HTTP/1.1\r\nHost: chat.ai.example:{port}\r\nConnection: close\r\n\r\n'.encode())
                assert reached.wait(5), 'Target not reached over the test proxy'
                connections = []
                deadline = time.monotonic() + 3
                while time.monotonic() < deadline:
                    connections = [c for c in api_get().get('connections', []) if c.get('metadata', {}).get('host') == 'chat.ai.example']
                    if connections:
                        break
                    time.sleep(.05)
                assert connections, 'No matching live connection in controller'
                chains = connections[0].get('chains', [])
                enabled = '-disabled' not in fixture.stem
                assert ('Policy' in chains) == enabled, (engine, fixture.name, chains)
                print(json.dumps({'engine': engine, 'fixture': fixture.name, 'chains': chains, 'rule': connections[0].get('rule')}, ensure_ascii=False), flush=True)
                release.set()
                assert b'200' in client.recv(4096), 'Request failed'
            finally:
                release.set()
                if client:
                    client.close()
                if child.poll() is None:
                    child.terminate()
                try:
                    child.wait(3)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
                server.shutdown()
                server.server_close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--fixtures', type=pathlib.Path, required=True)
    parser.add_argument('--mihomo', type=pathlib.Path, required=True)
    parser.add_argument('--singbox', type=pathlib.Path, required=True)
    options = parser.parse_args()
    for name, binary in [('mihomo', options.mihomo), ('singbox', options.singbox)]:
        for suffix in ['', '-disabled']:
            verify(binary.resolve(), name, options.fixtures / f'{name}{suffix}.json')
