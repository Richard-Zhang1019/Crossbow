//! WinINET per-connection API: update effective settings, not just registry mirrors.
use super::{ProxyEndpoint, ServiceSnapshot, SysProxyBackend, WindowsProxyConfig};
use std::mem::size_of;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::GlobalFree;
use windows_sys::Win32::Networking::WinInet::*;
// #458：sysproxy.rs 的归属核对需要区分 PAC/WPAD 自动配置位；glob 导入
// 不会向上层可见，显式再导出给父模块。
pub(super) use windows_sys::Win32::Networking::WinInet::{
    PROXY_TYPE_AUTO_DETECT, PROXY_TYPE_AUTO_PROXY_URL,
};

pub struct WinInet;

impl WinInet {
    fn query(flags_option: u32) -> Result<WindowsProxyConfig, String> {
        let mut options = [
            flags_option,
            INTERNET_PER_CONN_PROXY_SERVER,
            INTERNET_PER_CONN_PROXY_BYPASS,
            INTERNET_PER_CONN_AUTOCONFIG_URL,
        ]
        .map(|dwOption| INTERNET_PER_CONN_OPTIONW {
            dwOption,
            Value: INTERNET_PER_CONN_OPTIONW_0 {
                pszValue: null_mut(),
            },
        });
        let mut list = INTERNET_PER_CONN_OPTION_LISTW {
            dwSize: size_of::<INTERNET_PER_CONN_OPTION_LISTW>() as u32,
            pszConnection: null_mut(),
            dwOptionCount: options.len() as u32,
            dwOptionError: 0,
            pOptions: options.as_mut_ptr(),
        };
        let mut length = list.dwSize;
        unsafe {
            let ok = InternetQueryOptionW(
                null(),
                INTERNET_OPTION_PER_CONNECTION_OPTION,
                (&mut list as *mut INTERNET_PER_CONN_OPTION_LISTW).cast(),
                &mut length,
            );
            let error = std::io::Error::last_os_error();
            let mut strings = Vec::new();
            for option in &options[1..] {
                let ptr = option.Value.pszValue;
                if ptr.is_null() {
                    strings.push(String::new());
                    continue;
                }
                let mut len = 0;
                while *ptr.add(len) != 0 {
                    len += 1;
                }
                strings.push(String::from_utf16_lossy(std::slice::from_raw_parts(
                    ptr, len,
                )));
                GlobalFree(ptr.cast());
            }
            if ok == 0 {
                return Err(format!("读取 Windows 代理设置失败：{error}"));
            }
            Ok(WindowsProxyConfig {
                flags: options[0].Value.dwValue,
                server: strings.remove(0),
                bypass: strings.remove(0),
                pac_url: strings.remove(0),
            })
        }
    }

    fn current() -> Result<WindowsProxyConfig, String> {
        Self::query(INTERNET_PER_CONN_FLAGS_UI).or_else(|_| Self::query(INTERNET_PER_CONN_FLAGS))
    }

    fn apply(config: &WindowsProxyConfig) -> Result<(), String> {
        let wide = |value: &str| value.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let mut server = wide(&config.server);
        let mut bypass = wide(&config.bypass);
        let mut pac = wide(&config.pac_url);
        let mut options = [
            INTERNET_PER_CONN_OPTIONW {
                dwOption: INTERNET_PER_CONN_FLAGS,
                Value: INTERNET_PER_CONN_OPTIONW_0 {
                    dwValue: config.flags,
                },
            },
            INTERNET_PER_CONN_OPTIONW {
                dwOption: INTERNET_PER_CONN_PROXY_SERVER,
                Value: INTERNET_PER_CONN_OPTIONW_0 {
                    pszValue: server.as_mut_ptr(),
                },
            },
            INTERNET_PER_CONN_OPTIONW {
                dwOption: INTERNET_PER_CONN_PROXY_BYPASS,
                Value: INTERNET_PER_CONN_OPTIONW_0 {
                    pszValue: bypass.as_mut_ptr(),
                },
            },
            INTERNET_PER_CONN_OPTIONW {
                dwOption: INTERNET_PER_CONN_AUTOCONFIG_URL,
                Value: INTERNET_PER_CONN_OPTIONW_0 {
                    pszValue: pac.as_mut_ptr(),
                },
            },
        ];
        let list = INTERNET_PER_CONN_OPTION_LISTW {
            dwSize: size_of::<INTERNET_PER_CONN_OPTION_LISTW>() as u32,
            pszConnection: null_mut(),
            dwOptionCount: options.len() as u32,
            dwOptionError: 0,
            pOptions: options.as_mut_ptr(),
        };
        unsafe {
            if InternetSetOptionW(
                null(),
                INTERNET_OPTION_PER_CONNECTION_OPTION,
                (&list as *const INTERNET_PER_CONN_OPTION_LISTW).cast(),
                list.dwSize,
            ) == 0
            {
                return Err(format!(
                    "设置 Windows 代理失败：{}",
                    std::io::Error::last_os_error()
                ));
            }
            for option in [INTERNET_OPTION_SETTINGS_CHANGED, INTERNET_OPTION_REFRESH] {
                if InternetSetOptionW(null(), option, null(), 0) == 0 {
                    return Err(format!(
                        "刷新 Windows 代理失败：{}",
                        std::io::Error::last_os_error()
                    ));
                }
            }
        }
        Ok(())
    }
}

impl SysProxyBackend for WinInet {
    fn list_services(&self) -> Result<Vec<String>, String> {
        Ok(vec!["Windows".into()])
    }

    fn enable_service(&self, _: &str, port: u16) -> Result<(), String> {
        let mut bypass = vec![
            "<local>".to_string(),
            "localhost".into(),
            "127.*".into(),
            "10.*".into(),
            "192.168.*".into(),
            "169.254.*".into(),
            "*.local".into(),
        ];
        bypass.extend((16..=31).map(|part| format!("172.{part}.*")));
        let config = WindowsProxyConfig {
            flags: PROXY_TYPE_DIRECT | PROXY_TYPE_PROXY,
            server: format!("http=127.0.0.1:{port};https=127.0.0.1:{port};socks=127.0.0.1:{port}"),
            bypass: bypass.join(";"),
            pac_url: String::new(),
        };
        Self::apply(&config)?;
        let actual = Self::current()?;
        let expected = Some(ProxyEndpoint {
            host: "127.0.0.1".into(),
            port,
        });
        if actual.endpoint("http") != expected
            || actual.endpoint("https") != expected
            || actual.flags & (PROXY_TYPE_AUTO_PROXY_URL | PROXY_TYPE_AUTO_DETECT) != 0
        {
            return Err(
                "Windows 代理设置未生效或被其他策略覆盖，请检查系统策略及其他代理程序".into(),
            );
        }
        Ok(())
    }

    fn disable_service(&self, _: &str) -> Result<(), String> {
        let mut config = Self::current()?;
        config.flags = PROXY_TYPE_DIRECT;
        Self::apply(&config)
    }

    fn service_snapshot(&self, _: &str) -> Result<ServiceSnapshot, String> {
        let config = Self::current()?;
        Ok(ServiceSnapshot {
            web: config.endpoint("http"),
            secure: config.endpoint("https"),
            socks: config.endpoint("socks"),
            bypass: Some(config.bypass.clone()),
            windows: Some(config),
        })
    }

    fn restore_service(&self, _: &str, snapshot: &ServiceSnapshot) -> Result<(), String> {
        if let Some(config) = &snapshot.windows {
            Self::apply(config)?;
            if Self::current()? != *config {
                return Err("Windows 系统代理恢复结果与快照不一致".into());
            }
            return Ok(());
        }
        // Older journals did not contain PAC/WPAD fields. Preserve compatibility.
        let mut entries = Vec::new();
        for (protocol, endpoint) in [
            ("http", &snapshot.web),
            ("https", &snapshot.secure),
            ("socks", &snapshot.socks),
        ] {
            if let Some(ep) = endpoint {
                entries.push(format!("{protocol}={}:{}", ep.host, ep.port));
            }
        }
        Self::apply(&WindowsProxyConfig {
            flags: if entries.is_empty() {
                PROXY_TYPE_DIRECT
            } else {
                PROXY_TYPE_DIRECT | PROXY_TYPE_PROXY
            },
            server: entries.join(";"),
            bypass: snapshot.bypass.clone().unwrap_or_default(),
            pac_url: String::new(),
        })
    }
}
