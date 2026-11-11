// 自动生成自 lib.rs 的 command 分域提取（#423）。use crate::* 引入根模块项。
use crate::*;
use tauri::{AppHandle, Emitter, Manager, State};

/// 切换运行模式（经控制器 PATCH，运行时生效）。
#[tauri::command]
pub(crate) fn set_core_mode(
    app: AppHandle,
    state: State<AppState>,
    mode: String,
) -> Result<(), String> {
    if let Some((port, secret)) = state.core().controller() {
        crate::engine::api::Controller { port, secret }.patch_mode(&mode)?;
    }
    *state.mode.lock().unwrap_or_else(|e| e.into_inner()) = mode.clone();
    let _ = app.emit("core://mode", mode);
    refresh_tray(&app);
    Ok(())
}
#[tauri::command]
pub(crate) fn core_mode(state: State<AppState>) -> String {
    state.mode.lock().unwrap_or_else(|e| e.into_inner()).clone()
}
/// 内核版本（执行 `<bin> -v` 首行）。
#[tauri::command]
pub(crate) fn core_version(state: State<AppState>) -> Result<String, String> {
    let bin = state.core().binary_path();
    if !bin.exists() {
        return Err("内核未安装".into());
    }
    let out = crate::process::command(&bin)
        .arg("-v")
        .output()
        .map_err(|e| e.to_string())?;
    let first = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if first.is_empty() {
        Err("无法读取内核版本".into())
    } else {
        Ok(first)
    }
}
/// 下载安装内核（前端用 invoke 阻塞调用，期间显示进度提示）。
#[tauri::command]
pub(crate) async fn core_install(
    app: AppHandle,
    state: State<'_, AppState>,
    engine: Option<String>,
) -> Result<CoreBinaryInfo, String> {
    let _request = state
        .core_install_op
        .try_lock()
        .map_err(|_| "内核正在下载")?;
    let mut notice = notifications::Notice::new(
        notifications::Kind::Progress,
        "core.install",
        "core-download",
    );
    notice.description = "preparing".into();
    notifications::publish(&app, notice);
    let handle = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || core_install_inner(handle, engine))
        .await
        .map_err(|e| e.to_string())
        .and_then(|result| result);
    if let Err(error) = &result {
        let mut notice = notifications::Notice::new(
            notifications::Kind::Error,
            "core.installFailed",
            "core-download",
        );
        notice.details = error.clone();
        notice.actions = vec![notifications::Action::Settings];
        notifications::publish(&app, notice);
    } else {
        notifications::result(&app, "core-download", "core.installReady", Ok(()));
    }
    result
}
#[tauri::command]
pub(crate) fn core_binary_info(state: State<AppState>) -> CoreBinaryInfo {
    let resolved = resolve_core_binary(&state.data_dir);
    let path = resolved
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let source = match &resolved {
        None => "missing",
        Some(p) if p.starts_with(&state.data_dir) => "data",
        Some(_) => "builtin",
    };
    let version = resolved
        .as_ref()
        .and_then(core_version_of)
        .unwrap_or_default();
    CoreBinaryInfo {
        path,
        source: source.into(),
        version,
    }
}
/// 按引擎查询内核安装状态。
#[tauri::command]
pub(crate) async fn core_binary_info_for(
    state: State<'_, AppState>,
    engine: String,
) -> Result<CoreBinaryInfo, String> {
    let resolved = crate::engine::download::resolve_core_for(&state.data_dir, Some(&engine));
    let path = resolved
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let source = match &resolved {
        None => "missing",
        Some(p) if p.starts_with(&state.data_dir) => "data",
        Some(_) => "builtin",
    };
    let version = resolved
        .as_ref()
        .and_then(core_version_of)
        .unwrap_or_default();
    Ok(CoreBinaryInfo {
        path,
        source: source.into(),
        version,
    })
}
#[tauri::command]
pub(crate) fn core_status(state: State<AppState>) -> CoreStatus {
    state.core().status()
}
#[tauri::command]
pub(crate) async fn core_stop(state: State<'_, AppState>) -> Result<(), String> {
    let _operation = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
    state.exit.ensure_open()?;
    if state.core().stop() {
        Ok(())
    } else {
        Err("内核未退出，请重试停止".into())
    }
}
#[tauri::command]
pub(crate) async fn core_restart(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let _request = state
        .core_restart_op
        .try_lock()
        .map_err(|_| "内核正在重启")?;
    notifications::publish(
        &app,
        notifications::Notice::new(
            notifications::Kind::Progress,
            "core.restarting",
            "core-restart",
        ),
    );
    let handle = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<AppState>();
        let _operation = state.core_op.lock().unwrap_or_else(|e| e.into_inner());
        state.exit.ensure_open()?;
        if !state.core().stop() {
            return Err("旧内核未退出，已中止重启".into());
        }
        start_core_locked(&state).and_then(|()| wait_core_running(&state))
    })
    .await
    .map_err(|error| error.to_string())
    .and_then(|result| result);
    let code = if result.is_ok() {
        "core.restarted"
    } else {
        "core.restartFailed"
    };
    notifications::result(&app, "core-restart", code, result.clone());
    result
}
#[tauri::command]
pub(crate) async fn core_start(state: State<'_, AppState>) -> Result<(), String> {
    start_core(&state).and_then(|()| wait_core_running(&state))
}
/// 节点旗帜补全开关。
/// 切换内核引擎（mihomo / singbox）。运行中则热重启；切换目标未安装则报错。
#[tauri::command]
pub(crate) async fn set_engine(
    app: AppHandle,
    state: State<'_, AppState>,
    engine: String,
) -> Result<(), String> {
    let target = match engine.as_str() {
        "mihomo" => crossbow_core::Engine::Mihomo,
        "singbox" => crossbow_core::Engine::SingBox,
        _ => return Err(format!("unknown engine: {engine}")),
    };
    let need_bin = match target {
        crossbow_core::Engine::SingBox => "sing-box",
        crossbow_core::Engine::Mihomo => "mihomo",
    };
    if !crate::engine::download::resolve_core_for(&state.data_dir, Some(&engine)).is_some() {
        return Err(format!("内核 {need_bin} 未安装，请先在设置页下载"));
    }
    // 新引擎写进存储的时机在内核确认就绪之后。失败会退回旧引擎并拉起旧核。
    commit_config_change(&state, |data| {
        data.engine.engine = target;
        Ok(())
    })?;
    state.core().log_decision(&format!(
        "set_engine: -> {engine} status={:?}",
        state.core().status()
    ));
    refresh_tray(&app);
    // 通知侧栏 footer 等跟随引擎的 UI 重新取标识
    let _ = app.emit("engine://changed", engine.clone());
    Ok(())
}
