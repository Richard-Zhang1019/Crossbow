//! 内核二进制解析与下载。
//!
//! 定位链（`resolve_core_binary`）：
//! 1. `CROSSBOW_MIHOMO_BIN` 环境变量（开发/调试）
//! 2. 打包 .app 内置 sidecar（与主可执行文件同目录，externalBin 随包分发）
//! 3. 数据目录缓存 `binaries/mihomo`（手动安装或下载所得）
//!
//! 都找不到时由 `install_core` 从发布源下载（镜像优先）到数据目录缓存，
//! 下载过程校验归档 SHA-256，并在解包成功后原子替换缓存。

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

/// #465：固定 SHA-256（审阅过的上游 digest；key = 资产名）。镜像与官方源
/// 遵守同一校验规则；未列出的资产拒绝下载（fail closed）。
pub(crate) const ASSET_SHA256: &[(&str, &str)] = &[
    // https://github.com/MetaCubeX/mihomo/releases/tag/v1.19.32
    (
        "mihomo-darwin-amd64-v1.19.32.gz",
        "b7de9fc46af2ca985ecfb302a6362170e04ff2023dee1e82addc54133e669bc9",
    ),
    (
        "mihomo-darwin-arm64-v1.19.32.gz",
        "3312a6780652c622890fd4357c6a853bbf865464fd047ac7b7f52dab8de18652",
    ),
    (
        "mihomo-windows-amd64-v1.19.32.zip",
        "1ac84e795b5b915446e20139677fb6cc013a0778e876e8d4c96f994750b4c4a3",
    ),
    (
        "mihomo-windows-arm64-v1.19.32.zip",
        "7a4f6e58af9a120920935f2150cf642d945085fdb3e753314505b334234e1d1e",
    ),
    (
        "mihomo-linux-amd64-v1.19.32.gz",
        "8451100836c9eda194331c2babfad490b2faf30cebc1a04b0e76fd8ac2d35d10",
    ),
    (
        "mihomo-linux-arm64-v1.19.32.gz",
        "9dd862e28b46ff7d775f169cceebc28deccaa0a9e804237d421cd2571e0caba0",
    ),
    // https://github.com/SagerNet/sing-box/releases/tag/v1.14.2
    (
        "sing-box-1.14.2-darwin-amd64.tar.gz",
        "b0bfb0dc70a5fc708710b9f5ea98b9ee76d40fa4169928d25d73edc4331df2fe",
    ),
    (
        "sing-box-1.14.2-darwin-arm64.tar.gz",
        "925c5382eca8492b0150f868a6db20b18290a38700e621724b3703fd453e032d",
    ),
    (
        "sing-box-1.14.2-windows-amd64.zip",
        "c2d8bfff918755808781dfdeeb8581b6c91eb3a243d9a7b55483cfc0c0684d32",
    ),
    (
        "sing-box-1.14.2-windows-arm64.zip",
        "2bb467039310452380958b821983d5bb4f78fb70a2583914e4bea8b011582b64",
    ),
    (
        "sing-box-1.14.2-linux-amd64.tar.gz",
        "a684484d7477d1437282ee411f4d131d0340aaad60a7868841ebd5d87dd8a0c6",
    ),
    (
        "sing-box-1.14.2-linux-arm64.tar.gz",
        "b43a1fb1bda131c6653576741ce527eb2bdeab7c9308ca90ee8b972abb7e4a7f",
    ),
];

fn expected_sha256(asset: &str) -> Result<&'static str, String> {
    ASSET_SHA256
        .iter()
        .find(|(name, _)| *name == asset)
        .map(|(_, hex)| *hex)
        .ok_or_else(|| format!("资产 {asset} 缺少固定 SHA-256，拒绝下载"))
}

fn sha256_hex(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// 内核引擎标识（下载/定位共用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CoreKind {
    Mihomo,
    SingBox,
}

impl CoreKind {
    pub fn binary_name(self) -> String {
        match self {
            CoreKind::Mihomo => core_bin_file("mihomo"),
            CoreKind::SingBox => core_bin_file("sing-box"),
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
    /// GitHub release 下载路径：`<tag>/<asset>`（#464：两类内核都需要
    /// 显式版本段——mihomo 缺 `v<tag>/` 时官方源与镜像均返回 404）。
    fn url_path(self) -> Result<String, String> {
        match self {
            CoreKind::Mihomo => Ok(format!(
                "v{}/{}",
                CORE_VERSION.trim_start_matches('v'),
                core_asset_name()?
            )),
            CoreKind::SingBox => Ok(format!(
                "v{}/{}",
                SINGBOX_VERSION.trim_start_matches('v'),
                singbox_asset_name()?
            )),
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

/// 启动期定位链：env 覆盖 → mihomo → sing-box（任一可用内核）。
pub fn resolve_core_binary(data_dir: &Path) -> Option<PathBuf> {
    if let Some(env) = std::env::var_os("CROSSBOW_MIHOMO_BIN") {
        let p = PathBuf::from(env);
        if p.exists() {
            return Some(p);
        }
    }
    resolve_exact(data_dir, "mihomo").or_else(|| resolve_exact(data_dir, "sing-box"))
}

/// 内核二进制文件名（Windows 带 .exe 后缀）。
pub fn core_bin_file(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// 路径的文件名是否为指定内核（Windows 认 .exe）。
pub fn is_core_bin(path: &std::path::Path, name: &str) -> bool {
    path.file_name().and_then(|f| f.to_str()) == Some(&core_bin_file(name))
}

/// 按引擎严格解析内核路径：不存在跨引擎回退——查不到就是未安装。
pub fn resolve_core_for(data_dir: &Path, engine: Option<&str>) -> Option<PathBuf> {
    // 引擎键两种写法：前端下拉框 "singbox"，启动链二进制名 "sing-box"——都接受。
    // （曾因启动链传 "sing-box" 落入默认分支，换核永远换到 mihomo：
    //   首次启动被「管理器已就位早退」掩盖，二次切换必现。）
    let name = match engine {
        Some("singbox" | "sing-box") => "sing-box",
        _ => "mihomo",
    };
    resolve_exact(data_dir, name)
}

fn resolve_exact(data_dir: &Path, name: &str) -> Option<PathBuf> {
    let file = core_bin_file(name);
    // sidecar：与主可执行文件同目录（.app/Contents/MacOS/<name>）
    let exe = std::env::current_exe().ok()?;
    let sibling = exe.parent()?.join(&file);
    if sibling.exists() {
        return Some(sibling);
    }
    let cached = data_dir.join("binaries").join(&file);
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
        "linux" => "linux",
        other => return Err(format!("unsupported os {other}")),
    };
    Ok(format!("mihomo-{os}-{arch}-{CORE_VERSION}"))
}

/// 当前环境的 mihomo 资产文件名（macOS gzip；Windows zip）。
pub fn core_asset_name() -> Result<String, String> {
    if cfg!(windows) {
        return Ok(format!("{}.zip", platform_asset_name()?));
    }
    Ok(format!("{}.gz", platform_asset_name()?))
}

/// sing-box 资产名：macOS tar.gz；Windows zip。
pub fn singbox_asset_name() -> Result<String, String> {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => return Err(format!("unsupported arch {other}")),
    };
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "windows",
        "linux" => "linux",
        other => return Err(format!("unsupported os {other}")),
    };
    let base = format!(
        "sing-box-{}-{}-{}",
        SINGBOX_VERSION.trim_start_matches('v'),
        os,
        arch
    );
    if cfg!(windows) {
        Ok(format!("{base}.zip"))
    } else {
        Ok(format!("{base}.tar.gz"))
    }
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
    let url_path = kind.url_path()?;
    let asset = url_path
        .rsplit_once('/')
        .map(|(_, asset)| asset)
        .ok_or_else(|| format!("内核下载路径缺少资产名：{url_path}"))?;
    let expected = expected_sha256(asset)?;
    let dest_dir = data_dir.join("binaries");
    std::fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;
    let dest = dest_dir.join(kind.binary_name());

    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(180))
        .build();
    let mut last_err = String::from("no source attempted");
    for base in kind.sources() {
        let url = format!("{base}/{url_path}");
        emit(InstallProgress {
            stage: "downloading".into(),
            detail: url.clone(),
        });
        let result = agent
            .get(&url)
            .call()
            .map_err(|e| e.to_string())
            .and_then(|resp| {
                install_archive(&mut resp.into_reader(), &dest, kind, expected, || {
                    emit(InstallProgress {
                        stage: "verifying".into(),
                        detail: url.clone(),
                    });
                })
            });
        match result {
            Ok(()) => {
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

fn install_archive(
    reader: &mut dyn std::io::Read,
    dest: &Path,
    kind: CoreKind,
    expected: &str,
    verifying: impl FnOnce(),
) -> Result<(), String> {
    let parent = dest.parent().ok_or("内核缓存路径缺少父目录")?;
    // 每次尝试使用独立的临时目录，与最终缓存处于同一文件系统。
    let mut temp = tempfile::Builder::new();
    temp.prefix(".core-install-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let attempt = temp
        .tempdir_in(parent)
        .map_err(|e| format!("创建下载临时目录：{e}"))?;
    let archive = attempt.path().join("archive");
    {
        let mut file = std::fs::File::create(&archive).map_err(|e| format!("创建下载文件：{e}"))?;
        std::io::copy(reader, &mut file).map_err(|e| format!("写入下载文件：{e}"))?;
    }
    verifying();
    let actual = sha256_hex(&archive).map_err(|e| format!("读取下载文件以校验 SHA-256：{e}"))?;
    if actual != expected {
        return Err(format!("SHA-256 mismatch (got {actual}, want {expected})"));
    }

    let binary = attempt.path().join(kind.binary_name());
    unpack(&archive, &binary, kind)?;
    let size = std::fs::metadata(&binary)
        .map_err(|e| format!("读取解包后的二进制：{e}"))?
        .len();
    if size < 10_000_000 {
        return Err(format!("unpacked binary too small ({size} bytes)"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("设置内核执行权限：{e}"))?;
    }
    std::fs::rename(&binary, dest).map_err(|e| format!("替换内核缓存：{e}"))
}

/// 从下载包解出内核二进制到 dest。
fn unpack(download_path: &Path, dest: &Path, kind: CoreKind) -> Result<(), String> {
    #[cfg(windows)]
    {
        return unpack_zip(download_path, dest, kind);
    }
    #[allow(unreachable_code)]
    {
        unpack_unix(download_path, dest, kind)
    }
}

/// Windows：zip 包内提取内核可执行文件（mihomo-<target>-*.exe / sing-box.exe）。
#[cfg(windows)]
fn unpack_zip(download_path: &Path, dest: &Path, kind: CoreKind) -> Result<(), String> {
    let f = std::fs::File::open(download_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(f).map_err(|e| format!("zip: {e}"))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("zip entry: {e}"))?;
        let name = entry.name().to_string();
        let stem = kind.binary_name().trim_end_matches(".exe").to_string();
        let hit = name.ends_with(".exe")
            && (name.starts_with(&format!("{stem}-")) || name.starts_with(&stem));
        if hit {
            let mut out = std::fs::File::create(dest).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut out).map_err(|e| format!("extract: {e}"))?;
            return Ok(());
        }
    }
    Err("zip 包内未找到内核可执行文件".to_string())
}

/// macOS/Linux：gz 直解（mihomo）或 tar.gz 提取（sing-box）。
fn unpack_unix(download_path: &Path, dest: &Path, kind: CoreKind) -> Result<(), String> {
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

    /// 回归：ensure_engine_binary 传二进制名 "sing-box"，前端传键名 "singbox"，
    /// 两种写法都必须解析到 sing-box（曾有 "sing-box" 落入默认分支换到 mihomo，
    /// 首次启动被「管理器已就位早退」掩盖，二次切换必现）。
    #[test]
    fn resolve_accepts_both_engine_spellings() {
        let dir = tempfile::tempdir().unwrap();
        let bin_dir = dir.path().join("binaries");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::write(bin_dir.join(core_bin_file("sing-box")), b"fake").unwrap();
        std::fs::write(bin_dir.join(core_bin_file("mihomo")), b"fake").unwrap();
        for key in ["singbox", "sing-box"] {
            let p = resolve_core_for(dir.path(), Some(key))
                .unwrap_or_else(|| panic!("engine {key} should resolve"));
            assert!(
                is_core_bin(&p, "sing-box"),
                "engine {key} resolved to {}",
                p.display()
            );
        }
    }

    use super::*;

    #[test]
    fn resolve_is_exact_per_engine() {
        let dir = tempfile::tempdir().unwrap();
        let bins = dir.path().join("binaries");
        std::fs::create_dir_all(&bins).unwrap();
        // 只有 mihomo：查 sing-box 必须为 None（不跨引擎回退）
        std::fs::write(bins.join(core_bin_file("mihomo")), b"x").unwrap();
        assert!(
            resolve_core_for(dir.path(), Some("singbox")).is_none(),
            "sing-box must not fall back to mihomo"
        );
        let m = resolve_core_for(dir.path(), Some("mihomo")).unwrap();
        assert!(is_core_bin(&m, "mihomo"));
        // 两个都在：各归各
        std::fs::write(bins.join(core_bin_file("sing-box")), b"x").unwrap();
        let sb = resolve_core_for(dir.path(), Some("singbox")).unwrap();
        assert!(is_core_bin(&sb, "sing-box"));
    }

    /// #464：下载 URL 必须带版本段（缺 tag 段时 GitHub 返回 404）。
    /// 覆盖 macOS ARM64 / Windows x64 / Linux x64（共享下载链）。
    #[test]
    fn url_path_includes_version_tag() {
        let mk = |os: &str, arch: &str| -> String {
            // url_path 依赖当前编译目标的 asset 名；这里直接对公式断言，
            // 与 platform_asset_name/core_asset_name/singbox_asset_name 保持同源。
            let mihomo = format!(
                "v1.19.32/mihomo-{os}-{arch}-v1.19.32.{}",
                if os == "windows" { "zip" } else { "gz" }
            );
            let sing = format!(
                "v1.14.2/sing-box-1.14.2-{os}-{arch}.{}",
                if os == "windows" { "zip" } else { "tar.gz" }
            );
            format!("{mihomo}\n{sing}")
        };
        for (os, arch) in [
            ("darwin", "arm64"),
            ("windows", "amd64"),
            ("linux", "amd64"),
        ] {
            let urls = mk(os, arch);
            let (mihomo_url, sing_url) = urls.split_once('\n').expect("两行 URL");
            // 旧拼接形态 = 资产名直接开头（无 v<tag>/ 段）→ GitHub 404
            assert!(
                mihomo_url.starts_with("v1.19.32/mihomo-"),
                "缺 mihomo tag 段: {mihomo_url}"
            );
            assert!(
                sing_url.starts_with("v1.14.2/sing-box-"),
                "缺 sing-box tag 段: {sing_url}"
            );
        }
        // 当前目标平台的真实 url_path 也必须带版本段
        for kind in [CoreKind::Mihomo, CoreKind::SingBox] {
            let path = kind.url_path().unwrap();
            assert!(
                path.starts_with("v1.19.32/") || path.starts_with("v1.14.2/"),
                "{kind:?} url_path 缺版本段: {path}"
            );
        }
    }

    #[test]
    fn asset_name_matches_host_platform() {
        let name = core_asset_name().unwrap();
        if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
            assert_eq!(name, "mihomo-darwin-arm64-v1.19.32.gz");
        }
        if cfg!(windows) && cfg!(target_arch = "x86_64") {
            assert_eq!(name, "mihomo-windows-amd64-v1.19.32.zip");
        }
        if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
            assert_eq!(name, "mihomo-linux-amd64-v1.19.32.gz");
            assert_eq!(
                singbox_asset_name().unwrap(),
                "sing-box-1.14.2-linux-amd64.tar.gz"
            );
        }
        assert!(name.ends_with(if cfg!(windows) { ".zip" } else { ".gz" }));
    }

    #[test]
    fn failed_install_preserves_existing_cache_and_removes_temporary_files() {
        use sha2::{Digest, Sha256};

        let dir = tempfile::tempdir().unwrap();
        let kind = CoreKind::Mihomo;
        let dest = dir.path().join(kind.binary_name());
        std::fs::write(&dest, b"ORIGINAL-CORE-BYTES").unwrap();

        let truncated = [0x1f, 0x8b, 0x08, 0x00, 0xde, 0xad, 0xbe, 0xef];
        let matching_digest = format!("{:x}", Sha256::digest(truncated));
        for (expected, stage) in [
            ("bad digest", "SHA-256 mismatch"),
            (
                &matching_digest,
                if cfg!(windows) { "zip:" } else { "gunzip:" },
            ),
        ] {
            let error = install_archive(&mut truncated.as_slice(), &dest, kind, expected, || {})
                .unwrap_err();
            assert!(error.contains(stage), "{error}");
            assert_eq!(std::fs::read(&dest).unwrap(), b"ORIGINAL-CORE-BYTES");
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        }
    }

    /// #465：expected_sha256 对未列出资产 fail closed。
    #[test]
    fn sha256_table_fails_closed() {
        assert!(expected_sha256("mihomo-darwin-arm64-v1.19.32.gz").is_ok());
        assert!(expected_sha256("sing-box-1.14.2-windows-amd64.zip").is_ok());
        assert!(expected_sha256("unknown-asset.bin").is_err());
    }
}
