//! 插件宿主：插件发现与生命周期管理。
//!
//! 本模块负责**目录扫描与清单加载**；进程拉起与 RPC 收发见 [`crate::runner`]。

use std::path::{Path, PathBuf};

use crate::manifest::{Manifest, ValidationReport};
use crate::SUPPORTED_API_VERSION;

/// 一个已加载插件的描述（尚未启动进程）。
#[derive(Debug)]
pub struct PluginHandle {
    /// 插件清单。
    pub manifest: Manifest,
    /// 插件目录。
    pub dir: PathBuf,
    /// 是否已启用。
    pub enabled: bool,
}

/// 插件宿主。
#[derive(Debug, Default)]
pub struct PluginHost {
    /// 插件根目录。
    plugins_dir: Option<PathBuf>,
    /// 已加载插件。
    handles: Vec<PluginHandle>,
}

impl PluginHost {
    /// 创建一个空的宿主。
    pub fn new() -> Self {
        Self::default()
    }

    /// 绑定插件根目录。
    pub fn with_plugins_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.plugins_dir = Some(dir.into());
        self
    }

    /// 插件根目录。
    pub fn plugins_dir(&self) -> Option<&Path> {
        self.plugins_dir.as_deref()
    }

    /// 当前已加载插件数量。
    pub fn len(&self) -> usize {
        self.handles.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }

    /// 校验一个清单是否可被本宿主加载。
    pub fn validate(manifest: &Manifest) -> ValidationReport {
        manifest.validate(SUPPORTED_API_VERSION)
    }

    /// 扫描插件目录并加载清单。
    ///
    /// 逐个读取子目录下的 `manifest.toml`，解析并通过 [`PluginHost::validate`]
    /// 校验接口版本；通过者进入内存列表。清单解析失败的插件会被跳过并记录告警，
    /// **不会**导致整体失败（单个坏插件不应拖垮宿主）。
    pub fn discover(&mut self) -> anyhow::Result<usize> {
        let dir = self
            .plugins_dir
            .clone()
            .ok_or_else(|| anyhow::anyhow!("未绑定插件目录"))?;
        self.handles.clear();

        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => return Err(anyhow::anyhow!("读取插件目录失败 {}: {e}", dir.display())),
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let manifest_path = path.join("manifest.toml");
            if !manifest_path.exists() {
                continue;
            }
            let text = match std::fs::read_to_string(&manifest_path) {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!("插件清单读取失败 {}: {e}", manifest_path.display());
                    continue;
                }
            };
            let manifest: Manifest = match toml::from_str(&text) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!("插件清单解析失败 {}: {e}", manifest_path.display());
                    continue;
                }
            };
            let report = Self::validate(&manifest);
            if !report.ok {
                tracing::warn!(
                    "插件 {} 校验未通过，已跳过：{}",
                    manifest.plugin.id,
                    report.issues.join("; ")
                );
                continue;
            }
            if manifest.requires_risk_confirmation() {
                tracing::warn!(
                    "插件 {} 带有风险提示：{}",
                    manifest.plugin.id,
                    manifest.plugin.risk_note.clone().unwrap_or_default()
                );
            }
            self.handles.push(PluginHandle {
                manifest,
                dir: path,
                enabled: false,
            });
        }

        // 稳定排序，便于展示
        self.handles
            .sort_by(|a, b| a.manifest.plugin.id.cmp(&b.manifest.plugin.id));
        Ok(self.handles.len())
    }

    /// 列出已加载插件。
    pub fn list(&self) -> &[PluginHandle] {
        &self.handles
    }
}
