#!/usr/bin/env python3
"""Non-disruptive niri/Wayland acceptance using the actual Linux application.

Requires the smoke_desktop.py dependencies, niri, busctl, dbus-monitor and zenity. Only this
instance's windows, tray, private HOME and XDG directories are operated on.
"""
import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import signal
import shutil
import socket
import subprocess
import tempfile
import threading
import time
import urllib.parse

from smoke_desktop import Driver, free_port, network_fingerprint, wait_until


def bus(*args):
    output = subprocess.check_output(['busctl', '--user', '--json=short', '--', *args],
        text=True, stderr=subprocess.DEVNULL)
    return json.loads(output)['data'] if output.strip() else None


def owner_pid():
    lookup = ('call', 'org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus')
    name = 'com.richard.crossbow.SingleInstance'
    if not bus(*lookup, 'NameHasOwner', 's', name)[0]:
        return None
    try:
        return bus(*lookup, 'GetConnectionUnixProcessID', 's', name)[0]
    except subprocess.CalledProcessError:
        if not bus(*lookup, 'NameHasOwner', 's', name)[0]:
            return None  # The process exited between the two calls.
        raise


def windows(pid):
    return [w for w in json.loads(subprocess.check_output(['niri', 'msg', '--json', 'windows']))
            if w.get('pid') == pid]


def focused():
    return next((w['id'] for w in json.loads(subprocess.check_output(
        ['niri', 'msg', '--json', 'windows'])) if w.get('is_focused')), None)


def action(name, window):
    subprocess.run(['niri', 'msg', 'action', name, '--id', str(window)], check=True, capture_output=True)


def menu(pid):
    items = bus('get-property', 'org.kde.StatusNotifierWatcher', '/StatusNotifierWatcher',
                'org.kde.StatusNotifierWatcher', 'RegisteredStatusNotifierItems')
    for item in items:
        service, path = item.split('/', 1)
        try:
            item_pid = bus('call', 'org.freedesktop.DBus', '/org/freedesktop/DBus',
                           'org.freedesktop.DBus', 'GetConnectionUnixProcessID', 's', service)[0]
            if item_pid != pid:
                continue
            path = '/' + path
            target = bus('get-property', service, path, 'org.kde.StatusNotifierItem', 'Menu')
            return service, target
        except subprocess.CalledProcessError:
            continue
    return None


def menu_rows(target):
    revision, root = bus('call', *target, 'com.canonical.dbusmenu', 'GetLayout', 'iias', '0', '-1', '0')
    rows = []
    def visit(node):
        if isinstance(node, dict) and 'data' in node:
            node = node['data']
        item, props, children = node
        rows.append((item, {k: v.get('data') if isinstance(v, dict) else v for k, v in props.items()}))
        for child in children:
            visit(child)
    visit(root)
    return rows


def click_menu(target, label):
    # Menu rebuilds replace native item IDs asynchronously after config changes.
    for attempt in range(5):
        item = next(i for i, props in menu_rows(target) if props.get('label') == label)
        try:
            bus('call', *target, 'com.canonical.dbusmenu', 'Event', 'isvu', str(item), 'clicked', 'i', '0', '0')
            return
        except subprocess.CalledProcessError:
            if attempt == 4:
                raise
            time.sleep(.1)


def select_view(driver, overlay=False):
    for handle in driver.command('GET', '/window/handles'):
        driver.command('POST', '/window', {'handle': handle})
        if driver.js('return location.search.includes("notifications=1")') == overlay:
            return True
    return False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--application', required=True, type=Path)
    parser.add_argument('--driver', default='tauri-driver')
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    assert os.environ.get('XDG_SESSION_TYPE') == 'wayland' and os.environ.get('XDG_CURRENT_DESKTOP') == 'niri'
    assert owner_pid() is None, 'Leave the existing Crossbow instance running and test later'
    args.output.mkdir(parents=True, exist_ok=True)
    before, original_focus = network_fingerprint(), focused()
    report = {'passed': False, 'checks': [], 'application_sha256': hashlib.sha256(args.application.read_bytes()).hexdigest()}
    def record(name, passed, details=None):
        report['checks'].append({'name': name, 'passed': bool(passed), 'details': details})
        (args.output / 'report.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
        print(name, bool(passed), flush=True)
    driver = None
    child = None
    sentinel = None
    app_pid = None
    cold = None
    notifications = None
    temp = tempfile.TemporaryDirectory(prefix='crossbow-linux-daily-')
    env = os.environ.copy()
    for key in ['HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME']:
        env[key] = str(Path(temp.name) / key)
        Path(env[key]).mkdir()
    env['GDK_BACKEND'] = 'wayland'
    requests = []
    class Fixture(BaseHTTPRequestHandler):
        protocol_version = 'HTTP/1.1'
        def log_message(self, *_args): pass
        def do_CONNECT(self):
            assert self.path == 'crossbow-fixture.invalid:80'
            self.send_response(200); self.end_headers()
        def do_GET(self):
            requests.append(self.path)
            body = subscription.encode() if self.path.startswith('/subscription') else b'daily-fixture-ok'
            self.send_response(200); self.send_header('Content-Length', str(len(body)))
            self.end_headers(); self.wfile.write(body)
    server = ThreadingHTTPServer(('127.0.0.1', 0), Fixture)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    subscription = f'''proxies:
  - {{name: Daily A, type: http, server: 127.0.0.1, port: {server.server_port}}}
  - {{name: Daily B, type: http, server: 127.0.0.1, port: {server.server_port}}}
proxy-groups:
  - {{name: Daily, type: select, proxies: [Daily A, Daily B]}}
rules: ["MATCH,Daily"]
'''
    url = f'http://127.0.0.1:{server.server_port}/subscription'
    data = Path(env['XDG_DATA_HOME']) / 'com.richard.crossbow'
    assets = args.output.parent / 'fixture-assets'
    for name in ['geoip.metadb', 'geosite.dat']:
        source = assets / name
        if source.is_file():
            (data / 'runtime').mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, data / 'runtime' / name)
    def probe():
        result = subprocess.run(['curl', '--fail', '--silent', '--show-error', '--max-time', '8',
            '--noproxy', '', '--proxy', f'http://127.0.0.1:{mixed}',
            'http://crossbow-fixture.invalid/probe'], capture_output=True, text=True)
        return result.returncode == 0 and result.stdout == 'daily-fixture-ok'
    def log_size():
        return sum(p.stat().st_size for p in (data / 'logs').glob('*.log'))
    def reopen(target):
        click_menu(target, '打开主窗口')
        wait_until(lambda: any(w['title'] == 'Crossbow' for w in windows(app_pid)))
        wait_until(lambda: select_view(driver))
        wait_until(lambda: driver.js('return !!document.querySelector(".cb-home-page")'))
    def main_window():
        return next(w['id'] for w in windows(app_pid) if w['title'] == 'Crossbow')
    def reminder_count():
        return (args.output / 'notifications.log').read_text().count('member=Notify')
    try:
        # Restrict observation to Crossbow's generic reminders, never other apps.
        with (args.output / 'notifications.log').open('w') as log:
            notifications = subprocess.Popen(['dbus-monitor', '--session',
                "type='method_call',interface='org.freedesktop.Notifications',member='Notify',arg0='Crossbow'"],
                stdout=log, stderr=subprocess.DEVNULL)
        port, native_port, mixed = free_port(), free_port(), free_port()
        with (args.output / 'driver.log').open('w') as log:
            child = subprocess.Popen([args.driver, '--port', str(port), '--native-port', str(native_port)],
                                     env=env, stdout=log, stderr=subprocess.STDOUT)
        driver = Driver(port)
        def ready():
            try: return driver.request('GET', '/status')
            except OSError: return False
        wait_until(ready)
        response = driver.request('POST', '/session', {'capabilities': {'alwaysMatch': {
            'browserName': 'wry', 'tauri:options': {'application': str(args.application.resolve())}}}})
        driver.session = response['sessionId']
        driver.command('POST', '/timeouts', {'script': 180000})
        wait_until(lambda: driver.js('return !!document.querySelector(".cb-home-page")'))
        app_pid = owner_pid()
        record('native Wayland window', len(windows(app_pid)) == 1)
        driver.invoke('set_mixed_port', {'port': mixed})
        driver.invoke('import_profile_url', {'url': url, 'name': 'Daily acceptance'})
        profiles = driver.invoke('list_profiles')
        profile_id = profiles[0]['id']
        driver.invoke('set_active_profile', {'id': profile_id})
        driver.invoke('core_start')
        wait_until(lambda: driver.invoke('core_status')['state'] == 'Running', timeout=180)
        assets.mkdir(exist_ok=True)
        for name in ['geoip.metadb', 'geosite.dat']:
            source = data / 'runtime' / name
            if source.is_file():
                shutil.copyfile(source, assets / name)
        record('local explicit proxy', probe())
        tray = wait_until(lambda: menu(app_pid))
        labels = [p.get('label') for _, p in menu_rows(tray)]
        record('real tray registration and menu', '打开主窗口' in labels and any('退出' in (s or '') for s in labels), labels)
        for label, mode in [('直连', 'direct'), ('规则', 'rule')]:
            click_menu(tray, label)
            wait_until(lambda: driver.invoke('core_mode') == mode)
        record('tray mode selection reaches the running core', probe())
        driver.invoke('autostart_set', {'enable': True})
        entry = Path(env['HOME']) / '.config/autostart/Crossbow.desktop'
        record('autostart entry in isolated HOME', entry.is_file() and str(args.application.resolve()) in entry.read_text())
        driver.invoke('autostart_set', {'enable': False})
        record('autostart entry removal in isolated HOME', not entry.exists())
        # Ordinary close must retain the app, core and actual tray entry.
        driver.invoke('set_lightweight_close', {'enabled': False})
        for _ in range(3):
            action('close-window', main_window())
            wait_until(lambda: not any(w['title'] == 'Crossbow' for w in windows(app_pid)))
            assert probe()
            reopen(tray)
        record('three close-to-tray and reopen cycles', owner_pid() == app_pid and probe())
        # A second exact binary invocation must forward only to our known owner.
        warm = subprocess.run([str(args.application.resolve()), 'crossbow://import?url=' +
            urllib.parse.quote(url + '?warm', safe='')], env=env, capture_output=True, timeout=15)
        wait_until(lambda: len(driver.invoke('list_profiles')) >= 2)
        record('warm deep link import', warm.returncode == 0 and owner_pid() == app_pid)
        imported = next(p for p in driver.invoke('list_profiles') if p.get('url') == url + '?warm')
        click_menu(tray, imported['name'])
        wait_until(lambda: driver.invoke('active_profile_id') == imported['id'])
        click_menu(tray, profiles[0]['name'])
        wait_until(lambda: driver.invoke('active_profile_id') == profile_id)
        record('tray profile selection updates the active configuration', probe())
        driver.invoke('select_proxy', {'group': 'Daily', 'name': 'Daily B'})
        wait_until(lambda: any(g['name'] == 'Daily' and g['now'].endswith('Daily B')
                              for g in driver.invoke('proxies_snapshot')))
        driver.invoke('set_theme', {'theme': 'dark'})
        driver.invoke('set_logs_save', {'enabled': True})
        wait_until(lambda: probe() and log_size() > 0)
        # Keep a separate test-owned native window focused during notification delivery.
        sentinel = subprocess.Popen(['zenity', '--info', '--title=Crossbow Acceptance Focus',
            '--text=Crossbow focus acceptance', '--no-wrap'], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        sentinel_id = wait_until(lambda: next((w['id'] for w in windows(sentinel.pid)), None))
        for iteration in range(2):
            count = reminder_count()
            notice = driver.invoke('notification_publish', {'notice': {'kind': 'error',
                'title': 'Crossbow acceptance notice', 'details': 'token=focus-private-value',
                'source': f'daily-focus-{iteration}'}})
            action('close-window', main_window())
            action('focus-window', sentinel_id)
            before_focus = focused()
            already_sent = reminder_count() > count
            # No WebDriver operation occurs while native focus is observed.
            wait_until(lambda: reminder_count() > count)
            time.sleep(.5)
            record(f'background notice {iteration + 1} preserves focus',
                not already_sent and before_focus == sentinel_id and focused() == sentinel_id,
                {'before': before_focus, 'after': focused(),
                 'already_sent_before_observation': already_sent})
            record(f'background notice {iteration + 1} creates no toplevel', not windows(app_pid))
            reopen(tray)
            snapshot = driver.invoke('notification_snapshot')
            record(f'background notice {iteration + 1} retains unread history', any(
                n['id'] == notice and n['readAt'] is None and n['resolvedAt'] is None
                for n in snapshot['history']))
            driver.screenshot(args.output / f'notification-{iteration + 1}.png')
            driver.invoke('notification_dismiss', {'id': notice})
            time.sleep(.35)  # Let the foreground route withdraw the reminder.
        payload = (args.output / 'notifications.log').read_text()
        record('system reminders contain no notification title or secret',
            'focus-private-value' not in payload and 'Crossbow acceptance notice' not in payload)
        # Lightweight close destroys main; background work must continue and reattach.
        driver.invoke('set_lightweight_close', {'enabled': True})
        history_size = len(driver.invoke('traffic_history'))
        action('close-window', main_window())
        wait_until(lambda: not any(w['title'] == 'Crossbow' for w in windows(app_pid)))
        time.sleep(1)  # Exclude messages already buffered before the close.
        size = log_size()
        for _ in range(3):
            assert probe(); time.sleep(.5)
        background_size = log_size()
        reopen(tray)
        assert probe(); time.sleep(4)
        record('lightweight reopen retains background traffic samples',
               len(driver.invoke('traffic_history')) >= history_size + 3)
        record('lightweight close keeps background log persistence', background_size > size)
        size = log_size()
        for _ in range(3):
            assert probe(); time.sleep(.5)
        record('lightweight reopen restores log persistence', log_size() > size)
        # Lightweight close now leaves no WebView; native reminders must not exit the app.
        count = reminder_count()
        notice = driver.invoke('notification_publish', {'notice': {'kind': 'error',
            'title': 'Crossbow last-window acceptance', 'source': 'daily-last-window'}})
        action('close-window', main_window())
        wait_until(lambda: reminder_count() > count)
        time.sleep(1)
        survived = owner_pid() == app_pid
        record('native reminder without a WebView keeps app and proxy', survived and not windows(app_pid) and probe())
        if survived:
            reopen(tray)
            snapshot = driver.invoke('notification_snapshot')
            record('notification history survives lightweight native reminder', any(n['id'] == notice for n in snapshot['history']))
            quit_label = next(p['label'] for _, p in menu_rows(tray) if '退出' in p.get('label', ''))
            click_menu(tray, quit_label)
            wait_until(lambda: owner_pid() is None)
        # Release the automation session after the tested process has exited.
        try: driver.command('DELETE', '')
        except Exception: pass
        # Cold-start protocol delivery uses argv only; no system URL registration.
        cold_url = url + '?cold'
        with (args.output / 'cold.log').open('w') as log:
            cold = subprocess.Popen([str(args.application.resolve()), 'crossbow://import?url=' +
                urllib.parse.quote(cold_url, safe='')], env=env, stdout=log, stderr=subprocess.STDOUT)
        app_pid = wait_until(owner_pid)
        wait_until(lambda: windows(app_pid))
        time.sleep(3)
        record('cold deep link import', any(path.endswith('?cold') for path in requests))
        tray = wait_until(lambda: menu(app_pid))
        quit_label = next(p['label'] for _, p in menu_rows(tray) if '退出' in p.get('label', ''))
        click_menu(tray, quit_label)
        cold.wait(timeout=10)
        # A fresh GUI/core session must read the persisted ordinary-user state.
        response = driver.request('POST', '/session', {'capabilities': {'alwaysMatch': {
            'browserName': 'wry', 'tauri:options': {'application': str(args.application.resolve())}}}})
        driver.session = response['sessionId']
        driver.command('POST', '/timeouts', {'script': 180000})
        app_pid = wait_until(owner_pid)
        wait_until(lambda: driver.js('return !!document.querySelector(".cb-home-page")'))
        settings = driver.invoke('get_ui_settings')
        record('normal application restart preserves preferences and active profile',
            settings['theme'] == 'dark' and settings['lightweight_close']
            and driver.invoke('active_profile_id') == profile_id)
        driver.invoke('core_start')
        wait_until(lambda: driver.invoke('core_status')['state'] == 'Running', timeout=180)
        record('normal application restart preserves selected node', any(
            g['name'] == 'Daily' and g['now'].endswith('Daily B') for g in driver.invoke('proxies_snapshot')))
        record('restarted explicit proxy remains usable', probe())
        tray = wait_until(lambda: menu(app_pid))
        quit_label = next(p['label'] for _, p in menu_rows(tray) if '退出' in p.get('label', ''))
        click_menu(tray, quit_label)
        wait_until(lambda: owner_pid() is None)
        record('tray quit removes app PID file', not (data / 'app.pid').exists())
        def listener_closed():
            with socket.socket() as listener:
                return listener.connect_ex(('127.0.0.1', mixed)) != 0
        # D-Bus disappears before the process completes its Exit cleanup.
        try:
            wait_until(listener_closed, timeout=10)
            released = True
        except TimeoutError:
            released = False
        record('tray quit releases explicit listener', released)
    except Exception as error:
        report['error'] = str(error)
        raise
    finally:
        if driver and driver.session and owner_pid() == app_pid:
            try:
                driver.invoke('core_stop')
            except Exception:
                pass
        if owner_pid() == app_pid and app_pid:
            # Cleanup only the PID whose bus ownership was verified above.
            try:
                target = menu(app_pid)
                label = next(p['label'] for _, p in menu_rows(target) if '退出' in p.get('label', ''))
                click_menu(target, label)
            except Exception:
                os.kill(app_pid, signal.SIGTERM)
        for process in [cold, sentinel, child, notifications]:
            if process and process.poll() is None:
                process.terminate(); process.wait(timeout=10)
        # A failing UI/driver must not leave an orphan test core behind.
        for proc in Path('/proc').iterdir():
            if not proc.name.isdigit():
                continue
            try:
                if (proc / 'comm').read_text().strip() == 'mihomo' and any(
                    temp.name.encode() in arg for arg in (proc / 'cmdline').read_bytes().split(b'\0')):
                    os.kill(int(proc.name), signal.SIGTERM)
            except (OSError, ProcessLookupError):
                pass
        server.shutdown()
        if original_focus:
            subprocess.run(['niri', 'msg', 'action', 'focus-window', '--id', str(original_focus)], capture_output=True)
        report['network_unchanged'] = network_fingerprint() == before
        report['passed'] = not report.get('error') and report['network_unchanged'] and all(c['passed'] for c in report['checks'])
        (args.output / 'report.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
        temp.cleanup()
    if not report['passed']:
        raise SystemExit('Acceptance failed; inspect report.json')


if __name__ == '__main__':
    main()
