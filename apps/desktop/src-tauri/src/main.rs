// Prevents an extra console window on Windows in release builds.
// macOS 不受影响；保留以保持跨平台模板一致性。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    crossbow_desktop_lib::run()
}
