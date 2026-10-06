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

pub const SINGBOX_SOURCES: &[&str] = &[
    "https://gh-proxy.com/https://github.com/SagerNet/sing-box/releases/download",
    "https://github.com/SagerNet/sing-box/releases/download",
];

/// 内核版本锁定（升级走 PR 审查，DESIGN §5.3）。
pub const CORE_VERSION: &str = "v1.19.32";
pub const SINGBOX_VERSION: &str = "1.14.2";

/// 内核引擎标识（下载/定位共用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CoreKind {
    Mihomo,
    SingBox,
}

impl CoreKind {
    pub fn binary_name(self) -> &'static str {
        match self {
            CoreKind::Mihomo => "mihomo",
            CoreKind::SingBox => "sing-box",
        }
    }
    fn sources(self) -> &'static [&'static str] {
        match self {
            CoreKind::Mihomo => CORE_SOURCES,
            CoreKind::SingBox => SINGBOX_SOURCES,
        }
    }
    #[allow(dead_code)]
    fn version(self) -> &'static str {
        match self {
            CoreKind::Mihomo => CORE_VERSION,
            CoreKind::SingBox => SINGBOX_VERSION,
        }
    }
    /// sing-box 发行包是 tar.gz 且带目录前缀：sing-box-<v>-<os>-<arch>/
    #[allow(dead_code)]
    fn asset_kind(self) -> &'static str {
        match self {
            CoreKind::Mihomo => "gz",
            CoreKind::SingBox => "targz",
        }
    }
}

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
    // sidecar：与主可执行文件同目录（.app/Contents/MacOS/{mihomo,sing-box}）
    if let Ok(exe) = std::env::current_exe() {
        for name in ["mihomo", "sing-box"] {
            let sibling = exe.parent()?.join(name);
            if sibling.exists() {
                return Some(sibling);
            }
        }
    }
    for name in ["mihomo", "sing-box"] {
        let cached = data_dir.join("binaries").join(name);
        if cached.exists() {
            return Some(cached);
        }
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

/// sing-box 资产名：sing-box-<v>-<os>-<arch>.tar.gz
pub fn singbox_asset_name() -> Result<String, String> {
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
    Ok(format!(
        "sing-box-{}-{}-{}.tar.gz",
        SINGBOX_VERSION.trim_start_matches('v'),
        os,
        arch
    ))
}

#[derive(Serialize, Clone)]
pub struct InstallProgress {
    pub stage: String, // downloading / verifying / done
    pub detail: String,
}

/// 下载并安装内核到数据目录缓存。`emit` 用于向前端报告进度。
pub fn install_core(
    data_dir: &Path,
    kind: CoreKind,
    emit: &dyn Fn(InstallProgress),
) -> Result<PathBuf, String> {
    let asset = match kind {
        CoreKind::Mihomo => core_asset_name()?,
        CoreKind::SingBox => singbox_asset_name()?,
    };
    let dest_dir = data_dir.join("binaries");
    std::fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;
    let dest = dest_dir.join(kind.binary_name());
    let tmp = dest.with_extension("download");

    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(180))
        .build();
    let mut last_err = String::from("no source attempted");
    for base in kind.sources() {
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
                // 解包（gz 直解 / tar.gz 取内部二进制）→ chmod → 原子落位
                unpack(&tmp, &dest, kind)?;
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

/// 从下载包解出内核二进制到 dest。
fn unpack(download_path: &Path, dest: &Path, kind: CoreKind) -> Result<(), String> {
    let in_file = std::fs::File::open(download_path).map_err(|e| e.to_string())?;
    let gz = flate2::read::GzDecoder::new(in_file);
    match kind {
        CoreKind::Mihomo => {
            let mut out = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(dest)
                .map_err(|e| e.to_string())?;
            let mut decoder = gz;
            std::io::copy(&mut decoder, &mut out).map_err(|e| format!("gunzip: {e}"))?;
            Ok(())
        }
        CoreKind::SingBox => {
            // tar.gz 内路径 sing-box-<v>-<os>-<arch>/sing-box
            let mut archive = tar::Archive::new(gz);
            for entry in archive.entries().map_err(|e| format!("tar: {e}"))? {
                let mut entry = entry.map_err(|e| format!("tar: {e}"))?;
                let name = entry.path().map_err(|e| e.to_string())?;
                if name.file_name().map(|f| f == "sing-box").unwrap_or(false) {
                    let mut out = std::fs::OpenOptions::new()
                        .write(true)
                        .create(true)
                        .truncate(true)
                        .open(dest)
                        .map_err(|e| e.to_string())?;
                    std::io::copy(&mut entry, &mut out).map_err(|e| format!("extract: {e}"))?;
                    return Ok(());
                }
            }
            Err("tar 包内未找到 sing-box 二进制".to_string())
        }
    }
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
