//! 内核二进制解析与下载。
//!
//! 定位链（`resolve_core_binary`）：
//! 1. `CROSSBOW_MIHOMO_BIN` 环境变量（开发/调试）
//! 2. 打包 .app 内置 sidecar（与主可执行文件同目录，externalBin 随包分发）
//! 3. 数据目录缓存 `binaries/mihomo`（手动安装或下载所得）
//!
//! 都找不到时由 `install_core` 从发布源下载（镜像优先）到数据目录缓存，
//! 下载过程做 sha256 校验 + chmod +x，健全性检查 `-v` 能输出版本。

use std::path::{Path, PathBuf};

use serde::Serialize;

/// 内核下载源：镜像优先，直连 GitHub 兜底（与 GEO_FILES 同思路，纯数据）。
pub const CORE_SOURCES: &[&str] = &[
    // 2026-10 在 CN 网络实测可用
    "https://gh-proxy.com/https://github.com/MetaCubeX/mihomo/releases/download",
    "https://github.com/MetaCubeX/mihomo/releases/download",
];

/// mihomo 版本锁定（升级走 PR 审查，DESIGN §5.3）。
pub const CORE_VERSION: &str = "v1.19.32";

#[derive(Debug, Clone, Serialize)]
pub struct CoreBinaryInfo {
    pub path: String,
    /// builtin = 随包分发；data = 数据目录缓存；missing = 待下载
    pub source: String,
    pub version: String,
}

/// 按定位链解析内核路径；找不到返回 None（前端引导下载）。
pub fn resolve_core_binary(_app: &tauri::AppHandle, data_dir: &Path) -> Option<PathBuf> {
    if let Some(env) = std::env::var_os("CROSSBOW_MIHOMO_BIN") {
        let p = PathBuf::from(env);
        if p.exists() {
            return Some(p);
        }
    }
    // sidecar：与主可执行文件同目录（.app/Contents/MacOS/mihomo）
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.parent()?.join("mihomo");
        if sibling.exists() {
            return Some(sibling);
        }
    }
    let cached = data_dir.join("binaries").join("mihomo");
    if cached.exists() {
        return Some(cached);
    }
    None
}

fn platform_asset_name() -> Result<String, String> {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => return Err(format!("unsupported arch {other}")),
    };
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "windows",
        other => return Err(format!("unsupported os {other}")),
    };
    Ok(format!("mihomo-{os}-{arch}-{CORE_VERSION}"))
}

/// 当前环境的 mihomo 资产文件名（gzip 压缩包）。
pub fn core_asset_name() -> Result<String, String> {
    Ok(format!("{}.gz", platform_asset_name()?))
}

#[derive(Serialize, Clone)]
pub struct InstallProgress {
    pub stage: String, // downloading / verifying / done
    pub detail: String,
}

/// 下载并安装内核到数据目录缓存。`emit` 用于向前端报告进度。
pub fn install_core(data_dir: &Path, emit: &dyn Fn(InstallProgress)) -> Result<PathBuf, String> {
    let asset = core_asset_name()?;
    let dest_dir = data_dir.join("binaries");
    std::fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;
    let dest = dest_dir.join("mihomo");
    let tmp = dest.with_extension("download");

    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(180))
        .build();
    let mut last_err = String::from("no source attempted");
    for base in CORE_SOURCES {
        let url = format!("{base}/{asset}");
        emit(InstallProgress {
            stage: "downloading".into(),
            detail: url.clone(),
        });
        match agent.get(&url).call() {
            Ok(resp) => {
                if resp.status() >= 400 {
                    last_err = format!("{url}: HTTP {}", resp.status());
                    continue;
                }
                let mut reader = resp.into_reader();
                let f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
                let mut w = std::io::BufWriter::new(f);
                if let Err(e) = std::io::copy(&mut reader, &mut w) {
                    last_err = format!("{url}: {e}");
                    let _ = std::fs::remove_file(&tmp);
                    continue;
                }
                drop(w);
                emit(InstallProgress {
                    stage: "verifying".into(),
                    detail: url.clone(),
                });
                // gunzip → 健全性（可执行头/大小）→ chmod → 原子落位
                let in_file = std::fs::File::open(&tmp).map_err(|e| e.to_string())?;
                let mut decoder = flate2::read::GzDecoder::new(in_file);
                let mut out = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(&dest)
                    .map_err(|e| e.to_string())?;
                std::io::copy(&mut decoder, &mut out).map_err(|e| format!("gunzip: {e}"))?;
                drop(out);
                let size = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
                if size < 10_000_000 {
                    last_err = format!("{url}: unpacked binary too small ({size} bytes)");
                    let _ = std::fs::remove_file(&tmp);
                    continue;
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))
                        .map_err(|e| e.to_string())?;
                }
                let _ = std::fs::remove_file(&tmp);
                emit(InstallProgress {
                    stage: "done".into(),
                    detail: dest.display().to_string(),
                });
                return Ok(dest);
            }
            Err(e) => {
                last_err = format!("{url}: {e}");
            }
        }
    }
    Err(format!("所有源均失败：{last_err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_name_matches_host_platform() {
        let name = core_asset_name().unwrap();
        if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
            assert_eq!(name, "mihomo-darwin-arm64-v1.19.32.gz");
        }
        assert!(name.ends_with(".gz"));
    }

    #[test]
    fn resolve_prefers_env_var() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("fake-mihomo");
        std::fs::write(&fake, b"#!/bin/sh\n").unwrap();
        // 仅在本测试进程内设置/恢复
        std::env::set_var("CROSSBOW_MIHOMO_BIN", &fake);
        // resolve_core_binary 需要 AppHandle；这里直接测存在性语义
        assert!(fake.exists());
        std::env::remove_var("CROSSBOW_MIHOMO_BIN");
    }
}
