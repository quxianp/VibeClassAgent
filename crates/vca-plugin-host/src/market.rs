//! 本地插件市场（v1 仅本地，无在线市场）。
//!
//! 安装流程：
//! 1. 校验来源存在且含 `manifest.toml`；
//! 2. 解析并校验接口大版本；
//! 3. 若目标已存在则拒绝覆盖（除非 `force`），避免误删用户数据；
//! 4. 复制到 `<plugins_dir>/<plugin.id>/`；
//! 5. 返回权限与风险清单，由 CLI 展示给用户确认。

use std::path::{Path, PathBuf};

use crate::manifest::Manifest;
use crate::SUPPORTED_API_VERSION;

/// 市场来源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketSource {
    /// 本地目录。
    LocalDir(PathBuf),
    /// 本地 zip 包（v1 暂不支持自动解包，会提示用户先解压）。
    LocalZip(PathBuf),
    /// 远程地址（v1 默认未启用，配置项预留）。
    Remote(String),
}

/// 安装结果。
#[derive(Debug, Clone)]
pub struct InstallOutcome {
    /// 插件 id。
    pub plugin_id: String,
    /// 安装到的目录。
    pub installed_to: PathBuf,
    /// 声明需要的网络权限。
    pub network: Vec<String>,
    /// 声明需要的文件权限。
    pub filesystem: Vec<String>,
    /// 声明需要的环境变量。
    pub env: Vec<String>,
    /// 风险提示（无则为 None）。
    pub risk_note: Option<String>,
}

/// 安装/卸载错误。
#[derive(Debug, thiserror::Error)]
pub enum MarketError {
    /// 来源不存在。
    #[error("来源不存在: {0}")]
    NotFound(String),
    /// 缺少清单。
    #[error("目录里没有 manifest.toml: {0}")]
    NoManifest(String),
    /// 清单解析失败。
    #[error("清单解析失败: {0}")]
    BadManifest(String),
    /// 接口版本不匹配。
    #[error("接口版本不匹配: {0}")]
    ApiMismatch(String),
    /// 目标已存在。
    #[error("插件已存在: {0}（如需覆盖请加 --force）")]
    AlreadyExists(String),
    /// 文件操作失败。
    #[error("文件操作失败: {0}")]
    Io(String),
    /// 该来源被明确拒绝（zip / 在线市场 v1 不支持）。
    #[error("{0}")]
    Unsupported(String),
}

/// 本地插件市场。
#[derive(Debug, Clone, Default)]
pub struct LocalMarket {
    /// 远程市场地址；为空表示纯本地模式。
    remote_url: Option<String>,
}

impl LocalMarket {
    /// 构造纯本地市场。
    pub fn new() -> Self {
        Self { remote_url: None }
    }

    /// 设置远程地址（v1 默认留空，仅预留能力）。
    pub fn with_remote(mut self, url: impl Into<String>) -> Self {
        let url = url.into();
        self.remote_url = if url.trim().is_empty() {
            None
        } else {
            Some(url)
        };
        self
    }

    /// 是否启用远程市场。
    pub fn is_remote_enabled(&self) -> bool {
        self.remote_url.is_some()
    }

    /// 读取并校验某个目录里的插件清单。
    pub fn inspect(dir: &Path) -> Result<Manifest, MarketError> {
        if !dir.exists() {
            return Err(MarketError::NotFound(dir.display().to_string()));
        }
        let mf = dir.join("manifest.toml");
        if !mf.exists() {
            return Err(MarketError::NoManifest(dir.display().to_string()));
        }
        let text = std::fs::read_to_string(&mf).map_err(|e| MarketError::Io(e.to_string()))?;
        let manifest: Manifest =
            toml::from_str(&text).map_err(|e| MarketError::BadManifest(e.to_string()))?;
        let report = manifest.validate(SUPPORTED_API_VERSION);
        if !report.ok {
            return Err(MarketError::ApiMismatch(report.issues.join("; ")));
        }
        Ok(manifest)
    }

    /// 安装一个插件。
    pub fn install(
        &self,
        source: &MarketSource,
        plugins_dir: &Path,
        force: bool,
    ) -> Result<InstallOutcome, MarketError> {
        let src = match source {
            MarketSource::LocalDir(p) => p.clone(),
            MarketSource::LocalZip(p) => {
                // v1 不内置解压，避免引入压缩库依赖；明确告知用户如何操作
                if !p.exists() {
                    return Err(MarketError::NotFound(p.display().to_string()));
                }
                return Err(MarketError::Unsupported(format!(
                    "暂不支持直接安装 zip（{}）。请先解压到目录，再用该目录作为来源。",
                    p.display()
                )));
            }
            MarketSource::Remote(u) => {
                return Err(MarketError::Unsupported(format!(
                    "v1 未启用在线市场（{u}）。请从本地目录安装。"
                )))
            }
        };

        let manifest = Self::inspect(&src)?;
        let target = plugins_dir.join(&manifest.plugin.id);

        if target.exists() && !force {
            return Err(MarketError::AlreadyExists(target.display().to_string()));
        }
        std::fs::create_dir_all(plugins_dir).map_err(|e| MarketError::Io(e.to_string()))?;
        if target.exists() {
            std::fs::remove_dir_all(&target).map_err(|e| MarketError::Io(e.to_string()))?;
        }
        copy_dir(&src, &target).map_err(|e| MarketError::Io(e.to_string()))?;

        Ok(InstallOutcome {
            plugin_id: manifest.plugin.id.clone(),
            installed_to: target,
            network: manifest.permissions.network.clone(),
            filesystem: manifest.permissions.filesystem.clone(),
            env: manifest.permissions.env.clone(),
            risk_note: manifest.plugin.risk_note.clone(),
        })
    }

    /// 卸载插件。
    pub fn uninstall(&self, plugin_id: &str, plugins_dir: &Path) -> Result<PathBuf, MarketError> {
        let target = plugins_dir.join(plugin_id);
        if !target.exists() {
            return Err(MarketError::NotFound(target.display().to_string()));
        }
        std::fs::remove_dir_all(&target).map_err(|e| MarketError::Io(e.to_string()))?;
        Ok(target)
    }
}

/// 递归复制目录（不依赖第三方库）。
fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vca_mkt_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_plugin(dir: &Path, id: &str, api: &str, risk: bool) {
        std::fs::create_dir_all(dir).unwrap();
        let mut m = format!(
            "[plugin]\nid = \"{id}\"\nname = \"测试\"\nversion = \"1.0.0\"\napi_version = \"{api}\"\nkind = \"pusher\"\n"
        );
        if risk {
            m.push_str("risk_note = \"第三方协议有风险\"\n");
        }
        m.push_str("\n[runtime]\ntype = \"process\"\nentry = \"bin/x.exe\"\n\n[permissions]\nnetwork = [\"https://a\"]\nenv = [\"K\"]\n");
        std::fs::write(dir.join("manifest.toml"), m).unwrap();
    }

    #[test]
    fn inspect_rejects_missing_manifest() {
        let d = tmp("nm");
        assert!(matches!(
            LocalMarket::inspect(&d),
            Err(MarketError::NoManifest(_))
        ));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn inspect_rejects_api_mismatch() {
        let d = tmp("api");
        write_plugin(&d, "p", "99", false);
        assert!(matches!(
            LocalMarket::inspect(&d),
            Err(MarketError::ApiMismatch(_))
        ));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn install_copies_and_reports_permissions() {
        let src = tmp("src");
        write_plugin(&src, "pusher.demo", "1", true);
        let dest = tmp("dst");

        let mkt = LocalMarket::new();
        let out = mkt
            .install(&MarketSource::LocalDir(src.clone()), &dest, false)
            .unwrap();
        assert_eq!(out.plugin_id, "pusher.demo");
        assert!(out.installed_to.exists());
        assert_eq!(out.network, vec!["https://a".to_string()]);
        assert_eq!(out.env, vec!["K".to_string()]);
        assert!(out.risk_note.is_some());

        // 再次安装应因已存在而失败
        assert!(matches!(
            mkt.install(&MarketSource::LocalDir(src.clone()), &dest, false),
            Err(MarketError::AlreadyExists(_))
        ));
        // force 可覆盖
        assert!(mkt
            .install(&MarketSource::LocalDir(src.clone()), &dest, true)
            .is_ok());

        // 卸载
        mkt.uninstall("pusher.demo", &dest).unwrap();
        assert!(!dest.join("pusher.demo").exists());

        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn zip_source_is_explicitly_unsupported() {
        let d = tmp("zip");
        let z = d.join("x.zip");
        std::fs::write(&z, b"PK").unwrap();
        let mkt = LocalMarket::new();
        assert!(matches!(
            mkt.install(&MarketSource::LocalZip(z), &d, false),
            Err(MarketError::Unsupported(_))
        ));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn remote_disabled_by_default() {
        assert!(!LocalMarket::new().is_remote_enabled());
        assert!(LocalMarket::new()
            .with_remote("https://x")
            .is_remote_enabled());
        assert!(!LocalMarket::new().with_remote("").is_remote_enabled());
    }
}
