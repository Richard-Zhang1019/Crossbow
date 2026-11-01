#!/usr/bin/env python3
"""Exercise the real Linux WebKit/Tauri app with an isolated store and loopback fixture.

Requires tauri-driver, WebKitWebDriver, curl and a graphical session. No credentials,
system proxy, TUN, DNS or routes are changed. --upstream-port optionally verifies
external requests through an existing local HTTP proxy without reading its config.
"""
import argparse
import base64
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
import urllib.request
import urllib.error


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def wait_until(action, timeout=30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = action()
        if value:
            return value
        time.sleep(0.25)
    raise TimeoutError('Condition did not become true')


def network_fingerprint():
    values = []
    for args in [['ip', '-j', 'route', 'show', 'table', 'all'],
                 ['ip', '-6', '-j', 'route', 'show', 'table', 'all'],
                 ['ip', '-j', 'rule', 'show'], ['ip', '-6', '-j', 'rule', 'show']]:
        entries = json.loads(subprocess.check_output(args))
        for entry in entries:
            entry.pop('expires', None)
        values.append(entries)
    values.append(Path('/etc/resolv.conf').read_text())
    return hashlib.sha256(json.dumps(values, sort_keys=True).encode()).hexdigest()


class Driver:
    def __init__(self, port):
        self.base = f'http://127.0.0.1:{port}'
        self.session = ''
        self.http = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def request(self, method, path, payload=None):
        data = None if payload is None else json.dumps(payload).encode()
        req = urllib.request.Request(self.base + path, data=data, method=method,
                                     headers={'Content-Type': 'application/json'})
        try:
            with self.http.open(req, timeout=180) as response:
                value = json.load(response).get('value')
        except urllib.error.HTTPError as error:
            raise RuntimeError(error.read().decode()) from error
        if isinstance(value, dict) and value.get('error'):
            raise RuntimeError(value)
        return value

    def command(self, method, path, payload=None):
        return self.request(method, '/session/' + self.session + path, payload)

    def js(self, script, *args):
        return self.command('POST', '/execute/sync', {'script': script, 'args': list(args)})

    def ipc(self, command, args=None):
        result = self.command('POST', '/execute/async', {
            'script': 'const done = arguments[arguments.length - 1]; '
                      'window.__TAURI_INTERNALS__.invoke(arguments[0], arguments[1])'
                      '.then(value => done({value}), error => done({rejected:String(error)}));',
            'args': [command, args or {}],
        })
        return result

    def invoke(self, command, args=None):
        result = self.ipc(command, args)
        if 'rejected' in result:
            raise RuntimeError(f'{command}: {result["rejected"]}')
        return result.get('value')

    def element(self, css):
        return self.command('POST', '/element', {'using': 'css selector', 'value': css})[
            'element-6066-11e4-a52e-4f735466cecf']

    def click(self, css):
        self.command('POST', '/element/' + self.element(css) + '/click', {})

    def page(self, page):
        self.js('window.dispatchEvent(new CustomEvent("cb:goto", {detail:arguments[0]}))', page)
        wait_until(lambda: self.js('return !!document.querySelector(arguments[0])', '.cb-view-' + page))

    def screenshot(self, path):
        path.write_bytes(base64.b64decode(self.command('GET', '/screenshot')))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--application', required=True, type=Path)
    parser.add_argument('--driver', default='tauri-driver')
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--upstream-port', type=int)
    args = parser.parse_args()
    if args.upstream_port is not None and not 1 <= args.upstream_port <= 65535:
        parser.error('upstream port must be 1..65535')
    # The single-instance bus name is shared even with separate XDG directories.
    owner = subprocess.check_output([
        'gdbus', 'call', '--session', '--dest', 'org.freedesktop.DBus',
        '--object-path', '/org/freedesktop/DBus',
        '--method', 'org.freedesktop.DBus.NameHasOwner',
        'com.richard.crossbow.SingleInstance'], text=True).strip()
    if owner != '(false,)':
        parser.error('A Crossbow instance already owns the session bus name; leave it running and test later')
    args.output.mkdir(parents=True, exist_ok=True)
    report = {'passed': False, 'checks': [], 'desktop': os.getenv('XDG_CURRENT_DESKTOP'),
              'session_type': os.getenv('XDG_SESSION_TYPE'), 'gdk_backend': os.getenv('GDK_BACKEND'),
              'external_probe_via_existing_proxy': args.upstream_port is not None}
    before = network_fingerprint()
    fixture_requests = []
    subscription = {'content': '', 'broken': False}

    class Fixture(BaseHTTPRequestHandler):
        protocol_version = 'HTTP/1.1'

        def log_message(self, *_args):
            pass

        def do_CONNECT(self):
            # mihomo uses CONNECT for HTTP upstreams, even for cleartext targets.
            # Terminate only this fixture's tunnel; never open arbitrary sockets.
            if self.path != 'crossbow-fixture.invalid:80':
                self.send_error(502, 'Only the loopback fixture target is supported')
                return
            self.send_response(200, 'Connection established')
            self.end_headers()
            self.close_connection = False

        def do_GET(self):
            if self.path == '/subscription':
                body = b'not a configuration' if subscription['broken'] else subscription['content'].encode()
            else:
                fixture_requests.append(self.path)
                body = b'crossbow-linux-explicit-proxy-ok'
            self.send_response(200)
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    server = ThreadingHTTPServer(('127.0.0.1', 0), Fixture)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    fixture_port = server.server_port
    upstream = args.upstream_port or fixture_port
    subscription['content'] = f'''proxies:
  - {{name: Linux HTTP A, type: http, server: 127.0.0.1, port: {upstream}}}
  - {{name: Linux HTTP B, type: http, server: 127.0.0.1, port: {upstream}}}
proxy-groups:
  - name: Linux Test
    type: select
    proxies: [Linux HTTP A, Linux HTTP B]
rules:
  - MATCH,Linux Test
'''
    driver = None
    child = None
    temp = tempfile.TemporaryDirectory(prefix='crossbow-linux-smoke-')
    try:
        env = os.environ.copy()
        # Linux auto-launch uses HOME/.config directly, bypassing XDG_CONFIG_HOME.
        env['HOME'] = str(Path(temp.name) / 'home')
        Path(env['HOME']).mkdir()
        for key in ['XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME']:
            env[key] = str(Path(temp.name) / key)
            Path(env[key]).mkdir()
        # Only our driver and its children inherit these directories.
        port, native_port, mixed_port = free_port(), free_port(), free_port()
        log = (args.output / 'driver.log').open('w')
        child = subprocess.Popen([args.driver, '--port', str(port), '--native-port', str(native_port)],
                                 env=env, stdout=log, stderr=subprocess.STDOUT)
        driver = Driver(port)

        def ready():
            try:
                return driver.request('GET', '/status')
            except OSError:
                return False

        wait_until(ready)
        result = driver.request('POST', '/session', {'capabilities': {'alwaysMatch': {
            'browserName': 'wry', 'tauri:options': {'application': str(args.application.resolve())}}}})
        driver.session = result['sessionId']
        driver.command('POST', '/timeouts', {'script': 180000})
        wait_until(lambda: driver.js('return !!document.querySelector(".cb-home-page")'))
        assert driver.invoke('platform_capabilities') == {'system_proxy': False, 'tun': True, 'app_update': False}
        assert driver.js('return Array.from(document.querySelectorAll(".cb-takeover-row button")).every(b => b.disabled)')
        for command, params in [('sysproxy_toggle', {}), ('set_tun', {'enable': True})]:
            assert 'rejected' in driver.ipc(command, params)
        assert driver.invoke('core_status')['state'] == 'Stopped'
        report['checks'].append('system proxy and TUN without an active profile rejected before starting a core')
        for notice in driver.invoke('notification_snapshot')['active']:
            driver.invoke('notification_dismiss', {'id': notice['id']})
        driver.invoke('set_mixed_port', {'port': mixed_port})
        driver.page('profiles')
        driver.click('.cb-empty button')
        input_id = driver.element('#profile-url')
        driver.click('#profile-url')
        subscription_url = f'http://127.0.0.1:{fixture_port}/subscription'
        driver.command('POST', '/element/' + input_id + '/value', {
            'text': subscription_url, 'value': list(subscription_url)})
        wait_until(lambda: driver.js('return document.querySelector("#profile-url").value') == subscription_url)
        driver.click('.cb-dialog button[data-variant="primary"]')
        profiles = wait_until(lambda: driver.invoke('list_profiles'))
        profile_id = profiles[0]['id']
        driver.invoke('set_active_profile', {'id': profile_id})
        report['checks'].append('subscription imported through the UI and activated through real IPC')
        driver.page('home')
        wait_until(lambda: driver.js('return !!document.querySelector(arguments[0])', 'button[aria-label="显式代理"]:not(:disabled)'))
        driver.click('button[aria-label="显式代理"]')
        wait_until(lambda: driver.invoke('core_status')['state'] == 'Running', timeout=180)
        assert driver.js('const panel=document.querySelector(".cb-takeover-panel").getBoundingClientRect(); '
                         'const control=document.querySelector(".cb-mode-control").getBoundingClientRect(); '
                         'return control.bottom <= panel.bottom;'), 'takeover controls overflow their panel'
        driver.screenshot(args.output / 'home.png')
        report['checks'].append('explicit proxy started through the UI')
        # Only test the rejection path when read-only checks prove a conflict.
        # A clean host must never become a TUN acceptance test by accident.
        conflicts = driver.invoke('coexist_detect')
        if conflicts:
            result = driver.ipc('set_tun', {'enable': True})
            assert 'rejected' in result and '未接管网络' in str(result), result
            assert driver.invoke('core_status')['state'] == 'Running'
            assert not driver.invoke('tun_status')['enabled']
            report['checks'].append('existing proxy/VPN conflict rejected before authorization; explicit core retained')
            for notice in driver.invoke('notification_snapshot')['active']:
                driver.invoke('notification_dismiss', {'id': notice['id']})

        driver.page('settings')
        wait_until(lambda: '偏好设置' in driver.js('return document.body.innerText'))
        driver.js('Array.from(document.querySelectorAll("button")).find(b => b.textContent.trim() === "网络与内核").click()')
        expected_service = '网络服务已安装' if driver.invoke('tun_status')['ready'] else '尚未安装 Linux 网络服务'
        wait_until(lambda: expected_service in driver.js('return document.body.innerText'))
        assert '授权一次后内核升级前无需重复授权' not in driver.js('return document.body.innerText')
        driver.screenshot(args.output / 'settings.png')
        report['checks'].append('Linux settings distinguish service installation from session authorization')
        driver.page('proxies')
        wait_until(lambda: 'Linux HTTP B' in driver.js('return document.body.innerText'))
        node = driver.js('return Array.from(document.querySelectorAll(".cb-node-card")).find(e => e.textContent.includes("Linux HTTP B"))')
        driver.command('POST', '/element/' + node['element-6066-11e4-a52e-4f735466cecf'] + '/click', {})
        wait_until(lambda: any((g['now'] or '').endswith('Linux HTTP B') for g in driver.invoke('proxies_snapshot') if g['name'] == 'Linux Test'))
        driver.screenshot(args.output / 'nodes.png')
        report['checks'].append('UI node selection persisted to the live controller')
        driver.page('logs')
        time.sleep(1)
        target = 'http://www.gstatic.com/generate_204' if args.upstream_port else 'http://crossbow-fixture.invalid/probe'
        for scheme in ['http', 'socks5h']:
            result = subprocess.run(['curl', '--silent', '--show-error', '--max-time', '20',
                                     '--noproxy', '', '--proxy', f'{scheme}://127.0.0.1:{mixed_port}',
                                     '--write-out', '\n%{http_code}', target], capture_output=True, text=True, check=True)
            expected = '\n204' if args.upstream_port else 'crossbow-linux-explicit-proxy-ok\n200'
            assert result.stdout == expected, f'{scheme} proxy returned unexpected HTTP {result.stdout[-3:]}'
            report['checks'].append(f'{scheme} request traversed the real core')
        if not args.upstream_port:
            assert len(fixture_requests) >= 2
        else:
            result = subprocess.run(['curl', '--silent', '--show-error', '--max-time', '30',
                                     '--noproxy', '', '--proxy', f'http://127.0.0.1:{mixed_port}',
                                     '--output', '/dev/null', '--write-out', '%{http_code}',
                                     'https://github.com'], capture_output=True, text=True, check=True)
            assert result.stdout == '200'
            report['checks'].append('HTTPS CONNECT to github.com returned 200 via existing local proxy')
        wait_until(lambda: 'Linux Test' in driver.js('return document.body.innerText'))
        driver.screenshot(args.output / 'logs.png')
        report['checks'].append('real core traffic appeared in the logs page')
        subscription['broken'] = True
        assert 'rejected' in driver.ipc('update_profile', {'id': profile_id})
        assert driver.invoke('list_profiles')[0]['content'] == subscription['content']
        assert driver.invoke('core_status')['state'] == 'Running'
        report['checks'].append('invalid subscription refresh retained the last working configuration')
        subscription['broken'] = False
        original = subscription['content']
        for tun in ['tun: {enable: true}', 'tun: {enable: yes}',
                    'base: &base {enable: on}\ntun: {<<: *base}']:
            subscription['content'] = original + '\n' + tun + '\n'
            assert 'rejected' in driver.ipc('update_profile', {'id': profile_id}), tun
            assert driver.invoke('list_profiles')[0]['content'] == original
            assert driver.invoke('core_status')['state'] == 'Running'
        subscription['content'] = original
        report['checks'].append('TUN in subscription update rejected without replacing the running configuration')
        diagnosis = driver.invoke('run_diagnosis')
        proxy_check = next(c for c in diagnosis if c['id'] == 'sysproxy')
        assert proxy_check['skipped'] and not proxy_check['ok'] and proxy_check['fix'] is None
        assert all(c['ok'] for c in diagnosis if c['id'] not in ['sysproxy', 'proxied_traffic', 'dns_direct'])
        if args.upstream_port:
            assert all(c['ok'] for c in diagnosis if not c['skipped'])
        report['diagnosis'] = diagnosis
        driver.page('diagnostics')
        driver.click('.cb-diagnostics-page button[data-variant="primary"]')
        wait_until(lambda: '不适用' in driver.js('return document.body.innerText'))
        driver.screenshot(args.output / 'diagnostics.png')
        report['checks'].append('diagnostics display unsupported system proxy as not applicable')
        driver.page('home')
        driver.click('button[aria-label="显式代理"]')
        wait_until(lambda: driver.invoke('core_status')['state'] == 'Stopped')
        with socket.socket() as sock:
            assert sock.connect_ex(('127.0.0.1', mixed_port)) != 0
        report['checks'].append('UI stop released the mixed port')
        assert not driver.invoke('sysproxy_status')['enabled']
        assert not driver.invoke('tun_status')['enabled']
        report['passed'] = True
    except Exception as error:
        report['error'] = str(error)
        if driver and driver.session:
            try:
                driver.screenshot(args.output / 'failure.png')
                (args.output / 'failure.txt').write_text(driver.js('return document.body.innerText'))
            except Exception:
                pass
        raise
    finally:
        if driver and driver.session:
            try:
                driver.invoke('core_stop')
                driver.command('DELETE', '')
            except Exception:
                pass
        if child:
            child.terminate()
            child.wait(timeout=10)
        server.shutdown()
        report['network_unchanged'] = before == network_fingerprint()
        report['passed'] = report['passed'] and report['network_unchanged']
        (args.output / 'report.json').write_text(json.dumps(report, ensure_ascii=False, indent=2))
        temp.cleanup()
    if not report['passed']:
        raise RuntimeError('Network state changed during the run; inspect report.json')
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == '__main__':
    main()
