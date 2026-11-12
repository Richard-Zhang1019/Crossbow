//! 特权服务客户端（Windows）：命名管道 `\\.\pipe\crossbow-service` 的
//! 一问一答 JSON 协议，见 apps/service/src/main.rs 模块注释。
//! 服务由 NSIS 安装器注册（开机自启）；TUN 内核以 SYSTEM 运行必须经它
//! 拉起/停止——GUI 直接杀 SYSTEM 进程是 EPERM。

#[cfg(target_os = "windows")]
pub mod imp {
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_FILE_NOT_FOUND, ERROR_NO_DATA, ERROR_PIPE_BUSY,
        GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, ReadFile, WriteFile, OPEN_EXISTING, SECURITY_IMPERSONATION,
        SECURITY_SQOS_PRESENT,
    };
    /// 服务管道单次响应等待上限。
    const RESPONSE_DEADLINE: Duration = Duration::from_secs(15);
    use windows_sys::Win32::System::Pipes::{
        GetNamedPipeServerProcessId, SetNamedPipeHandleState, WaitNamedPipeW, PIPE_NOWAIT,
        PIPE_READMODE_BYTE,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    pub const PIPE_PATH: &str = "\\\\.\\pipe\\crossbow-service";
    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(Some(0)).collect()
    }
    fn service_exe() -> Result<PathBuf, String> {
        std::env::current_exe()
            .map_err(|e| e.to_string())?
            .parent()
            .map(|dir| dir.join("crossbow-service.exe"))
            .ok_or_else(|| "no install directory".into())
    }
    pub fn installed() -> bool {
        service_exe().is_ok_and(|path| path.is_file())
    }

    fn verify_server(pipe: HANDLE) -> Result<(), String> {
        let mut pid = 0;
        if unsafe { GetNamedPipeServerProcessId(pipe, &mut pid) } == 0 {
            return Err("cannot identify service".into());
        }
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return Err("cannot inspect service process".into());
        }
        let process = Handle(process);
        let mut buffer = vec![0u16; 32768];
        let mut len = buffer.len() as u32;
        if unsafe { QueryFullProcessImageNameW(process.0, 0, buffer.as_mut_ptr(), &mut len) } == 0 {
            return Err("cannot verify service image".into());
        }
        let image = PathBuf::from(OsString::from_wide(&buffer[..len as usize]))
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let expected = service_exe()?.canonicalize().map_err(|e| e.to_string())?;
        if !image
            .to_string_lossy()
            .eq_ignore_ascii_case(&expected.to_string_lossy())
        {
            return Err("service pipe is owned by an unexpected process".into());
        }
        Ok(())
    }
    fn connect_pipe() -> Result<Handle, String> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let handle = unsafe {
                CreateFileW(
                    wide(PIPE_PATH).as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    SECURITY_SQOS_PRESENT | SECURITY_IMPERSONATION,
                    std::ptr::null_mut(),
                )
            };
            if handle != INVALID_HANDLE_VALUE && !handle.is_null() {
                let handle = Handle(handle);
                verify_server(handle.0)?;
                let mode = PIPE_READMODE_BYTE | PIPE_NOWAIT;
                if unsafe {
                    SetNamedPipeHandleState(handle.0, &mode, std::ptr::null(), std::ptr::null())
                } == 0
                {
                    return Err("cannot configure service pipe".into());
                }
                return Ok(handle);
            }
            let code = unsafe { GetLastError() };
            if code != ERROR_FILE_NOT_FOUND && code != ERROR_PIPE_BUSY {
                return Err(format!("service connection failed ({code})"));
            }
            if Instant::now() >= deadline {
                return Err("Crossbow 服务未运行，请重新运行安装程序修复服务".into());
            }
            unsafe {
                WaitNamedPipeW(wide(PIPE_PATH).as_ptr(), 100);
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    }
    fn request(payload: serde_json::Value) -> Result<serde_json::Value, String> {
        let handle = connect_pipe()?;
        let out = format!("{payload}\n");
        if out.len() > 4096 {
            return Err("service request too large".into());
        }
        let mut written = 0;
        if unsafe {
            WriteFile(
                handle.0,
                out.as_ptr(),
                out.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        } == 0
            || written as usize != out.len()
        {
            return Err("pipe write failed".into());
        }
        let deadline = Instant::now() + RESPONSE_DEADLINE;
        let mut line = Vec::new();
        loop {
            if Instant::now() >= deadline {
                return Err("service response timeout".into());
            }
            let mut byte = 0u8;
            let mut read = 0;
            let ok = unsafe { ReadFile(handle.0, &mut byte, 1, &mut read, std::ptr::null_mut()) };
            if ok != 0 && read == 1 {
                if byte == b'\n' {
                    break;
                }
                if line.len() >= 4096 {
                    return Err("service response too large".into());
                }
                line.push(byte);
            } else if unsafe { GetLastError() } == ERROR_NO_DATA || (ok != 0 && read == 0) {
                std::thread::sleep(Duration::from_millis(10));
            } else {
                return Err("service disconnected before completing response".into());
            }
        }
        unsafe {
            WriteFile(
                handle.0,
                b"\n".as_ptr(),
                1,
                &mut written,
                std::ptr::null_mut(),
            );
        }
        serde_json::from_slice(&line).map_err(|e| format!("service response: {e}"))
    }

    fn unwrap_ok(response: serde_json::Value) -> Result<serde_json::Value, String> {
        if response.get("ok").and_then(|v| v.as_bool()) == Some(true) {
            Ok(response)
        } else {
            Err(response
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("service error")
                .to_string())
        }
    }

    /// 服务实际拉起的内核。描述文件和 external_binary 必须记这份路径，
    /// 不能记界面选中的数据目录副本或带平台后缀的侧车。
    pub fn pinned_core_path() -> Result<PathBuf, String> {
        let pinned = service_exe()?
            .parent()
            .ok_or("missing install dir")?
            .join("mihomo.exe");
        if !pinned.is_file() {
            return Err(
                "Windows TUN 需要完整安装包中随附的 mihomo 内核；当前缺少该文件，请修复安装".into(),
            );
        }
        pinned.canonicalize().map_err(|e| e.to_string())
    }

    /// 服务按设计只运行安装目录内的钉定 mihomo 副本（管理员保护）。引擎
    /// 解析出的内核可能是数据目录下载副本或旧版残留（升级后常见），但同属
    /// CI 钉版的 mihomo——TUN 会话统一改用钉定副本，不再要求路径逐字节相等
    /// （v0.10.3 的逐字节门会误伤所有非安装目录内核，#149）。
    pub fn make_core_spawner(
        binary: &Path,
    ) -> Result<impl FnOnce(&Path, &Path) -> Result<u32, String> + Send, String> {
        let selected = binary.canonicalize().map_err(|e| e.to_string())?;
        if !crate::engine::download::is_core_bin(&selected, "mihomo") {
            return Err(
                "Windows TUN 仅支持 mihomo 内核；当前引擎是 sing-box，请先在设置中切换".into(),
            );
        }
        let pinned = pinned_core_path()?;
        running()?;
        Ok(move |config: &Path, log: &Path| start_core(&pinned, config, log))
    }

    /// 让服务以 SYSTEM 拉起内核，返回 pid
    pub fn start_core(binary: &Path, config: &Path, log: &Path) -> Result<u32, String> {
        let path_str = |p: &Path| p.to_string_lossy().into_owned();
        let response = request(serde_json::json!({
            "cmd": "start",
            "binary": path_str(binary),
            "config": path_str(config),
            "log": path_str(log),
        }))?;
        let ok = unwrap_ok(response)?;
        ok.get("pid")
            .and_then(|v| v.as_u64())
            .map(|p| p as u32)
            .ok_or_else(|| "service returned no pid".into())
    }

    /// 让服务停掉其托管内核。服务无内核在跑时返回 Err（调用方回落到
    /// 常规用户态停止路径，避免误把普通内核标成已停）。
    pub fn stop_core() -> Result<(), String> {
        let response = request(serde_json::json!({"cmd": "stop"}))?;
        unwrap_ok(response).map(|_| ())
    }

    pub fn running() -> Result<Option<u32>, String> {
        let response = request(serde_json::json!({"cmd": "status"}))?;
        let ok = unwrap_ok(response)?;
        Ok(ok.get("pid").and_then(|v| v.as_u64()).map(|p| p as u32))
    }

    /// Elevated service installation belongs to the signed installer, never an
    /// arbitrary portable/user-writable executable path from the GUI.
    pub fn install_elevated() -> Result<(), String> {
        Err("Windows TUN 需要完整安装：请以管理员权限重新运行 Crossbow 安装程序完成安装".into())
    }
}

#[cfg(target_os = "windows")]
pub use imp::*;

#[cfg(not(target_os = "windows"))]
pub fn installed() -> bool {
    false
}
