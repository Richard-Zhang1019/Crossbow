// 自动生成自 lib.rs 的 command 分域提取（#423）。use crate::* 引入根模块项。
use crate::*;
use tauri::{AppHandle, Emitter, State};

#[tauri::command]
pub(crate) fn active_profile_id(state: State<AppState>) -> Option<String> {
    state.store.lock().unwrap().data().active_profile.clone()
}
#[tauri::command]
pub(crate) fn list_profiles(state: State<AppState>) -> Vec<Profile> {
    state.store.lock().unwrap().data().profiles.clone()
}
#[tauri::command]
pub(crate) fn remove_profile(
    app: AppHandle,
    state: State<AppState>,
    id: String,
) -> Result<(), String> {
    {
        let mut store = state.store.lock().unwrap();
        store.data_mut().remove_profile(&id)?;
        store.save().map_err(|e| e.to_string())?;
    }
    refresh_tray(&app);
    Ok(())
}
#[tauri::command]
pub(crate) async fn set_active_profile(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    commit_config_change(&state, |data| data.set_active_profile(&id))?;
    refresh_tray(&app);
    // 托盘/前端切换共用此命令：发事件让前端各处（页头配置名、概览卡片等）
    // 同步刷新——切换器自己的 DOM 事件只有页面内组件收得到（#357）。
    let _ = app.emit("profile://switched", id.as_str());
    Ok(())
}
/// 设置订阅自动更新间隔（分钟，0 = 关闭）。
#[tauri::command]
pub(crate) fn set_profile_interval(
    state: State<AppState>,
    id: String,
    interval_min: u32,
) -> Result<(), String> {
    let mut store = state.store.lock().unwrap();
    let p = store
        .data_mut()
        .profiles
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or("profile not found")?;
    p.update_interval_min = interval_min;
    store.save().map_err(|e| e.to_string())?;
    scheduler::clear_retry(&id);
    Ok(())
}
/// 重命名 Profile。
#[tauri::command]
pub(crate) fn rename_profile(
    app: AppHandle,
    state: State<AppState>,
    id: String,
    name: String,
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("名称不能为空".into());
    }
    {
        let mut store = state.store.lock().unwrap();
        let p = store
            .data_mut()
            .profiles
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or("profile not found")?;
        p.name = name.to_string();
        store.save().map_err(|e| e.to_string())?;
    }
    refresh_tray(&app);
    Ok(())
}
/// 刷新订阅并落库；正在运行则热重启应用新配置。
#[tauri::command]
pub(crate) async fn update_profile(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<Profile, String> {
    let profile = refresh_profile_blocking(&state, &id)?;
    scheduler::clear_retry(&id);
    notifications::resolve(&app, &format!("profile-auto-update-{id}"));
    refresh_tray(&app);
    Ok(profile)
}
/// 导入本地配置文件（文件选择器与拖拽导入共用）。
#[tauri::command]
pub(crate) async fn import_profile_file(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<Profile, String> {
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("读取文件失败 {path}: {e}"))?;
    let profile = import_content_blocking(&state, profile_name_from_path(&path), content)
        .map_err(|e| format!("{path}: {e}"))?;
    refresh_tray(&app);
    Ok(profile)
}
/// 导入本地/粘贴的配置内容。
#[tauri::command]
pub(crate) async fn import_profile_content(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    content: String,
) -> Result<Profile, String> {
    let profile = import_content_blocking(&state, name, content)?;
    refresh_tray(&app);
    Ok(profile)
}
/// 导入 URL 订阅：拉取 → 校验 → 落库。
#[tauri::command]
pub(crate) async fn import_profile_url(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
    name: Option<String>,
) -> Result<Profile, String> {
    let profile = import_url_blocking(&state, &url, name.as_deref())?;
    refresh_tray(&app);
    Ok(profile)
}
