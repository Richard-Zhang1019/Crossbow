// 自动生成自 lib.rs 的 command 分域提取（#423）。use crate::* 引入根模块项。
use crate::*;
use tauri::State;

#[tauri::command]
pub(crate) fn set_activity_reporting(state: State<AppState>, enabled: bool) -> Result<(), String> {
    state
        .activity_reporting
        .set_enabled(enabled)
        .map_err(|error| error.to_string())
}
#[tauri::command]
pub(crate) fn get_activity_reporting(state: State<AppState>) -> bool {
    state.activity_reporting.enabled()
}
