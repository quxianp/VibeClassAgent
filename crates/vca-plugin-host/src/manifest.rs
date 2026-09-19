//! 插件清单（`manifest.toml`）解析与校验。
//!
//! 对应策划书 4.2 的 manifest 示例。
//!
//! 清单文件的读取与校验见 [`crate::host::PluginHost::discover`] 与
//! [`crate::market::LocalMarket::inspect`]。
//! **尚未实现**：checksum 与签名的强制校验（v1 未启用）。

use serde::{Deserialize, Serialize};
use vca_ipc::PluginKind;

/// 插件清单。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// 插件元信息。
    pub plugin: PluginMeta,
    /// 运行时信息。
    pub runtime: RuntimeSpec,
    /// 权限声明（安装时向用户展示并求确认）。
    #[serde(default)]
    pub permissions: Permissions,
    /// 配置项 JSON Schema。
    #[serde(default)]
    pub config: serde_json::Value,
}

/// 插件元信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMeta {
    /// 插件 id，如 `pusher.wecom`。
    pub id: String,
    /// 显示名。
    pub name: String,
    /// 插件版本。
    pub version: String,
    /// 宿主接口大版本。
    pub api_version: String,
    /// 插件类型。
    pub kind: PluginKind,
    /// 作者。
    #[serde(default)]
    pub author: Option<String>,
    /// 许可证。
    #[serde(default)]
    pub license: Option<String>,
    /// 风险提示（第三方协议类插件必填，见策划书附录 C）。
    #[serde(default)]
    pub risk_note: Option<String>,
}

/// 运行时规格。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeSpec {
    /// 运行模式：`process` / `cdylib` / `wasm`。默认 process。
    #[serde(default = "default_runtime_type")]
    pub r#type: String,
    /// 入口可执行体（相对路径）。
    pub entry: String,
    /// 单次调用超时（秒）。
    #[serde(default = "default_timeout")]
    pub timeout_sec: u64,
    /// 失败重启策略。
    #[serde(default)]
    pub restart: Option<String>,
}

fn default_runtime_type() -> String {
    "process".to_string()
}

fn default_timeout() -> u64 {
    30
}

/// 权限声明。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Permissions {
    /// 允许访问的网络地址。
    #[serde(default)]
    pub network: Vec<String>,
    /// 允许访问的文件路径（读写）。
    #[serde(default)]
    pub filesystem: Vec<String>,
    /// 需要读取的环境变量名。
    #[serde(default)]
    pub env: Vec<String>,
}

/// 清单校验结果。
#[derive(Debug, Clone)]
pub struct ValidationReport {
    /// 是否通过。
    pub ok: bool,
    /// 问题列表。
    pub issues: Vec<String>,
}

impl Manifest {
    /// 校验接口版本与必填项。
    ///
    /// 目前检查：接口大版本、入口非空、运行模式合法。
    pub fn validate(&self, supported_api_version: &str) -> ValidationReport {
        let mut issues = Vec::new();

        if self.plugin.api_version != supported_api_version {
            issues.push(format!(
                "接口大版本不匹配：插件为 {}，宿主支持 {}",
                self.plugin.api_version, supported_api_version
            ));
        }
        if self.runtime.entry.trim().is_empty() {
            issues.push("运行时入口 entry 为空".to_string());
        }
        if self.runtime.r#type != "process"
            && self.runtime.r#type != "cdylib"
            && self.runtime.r#type != "wasm"
        {
            issues.push(format!("未知运行模式: {}", self.runtime.r#type));
        }

        ValidationReport {
            ok: issues.is_empty(),
            issues,
        }
    }

    /// 是否需要向用户展示风险确认。
    pub fn requires_risk_confirmation(&self) -> bool {
        self.plugin.risk_note.is_some()
    }
}

/// 插件运行时模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeType {
    /// 独立进程 + JSON-RPC over stdio（默认，隔离性最好）。
    Process,
    /// 动态库直接加载。
    Cdylib,
    /// WASI 沙箱。
    Wasm,
}

impl RuntimeType {
    /// 从字符串解析。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "process" => Some(Self::Process),
            "cdylib" => Some(Self::Cdylib),
            "wasm" => Some(Self::Wasm),
            _ => None,
        }
    }
}
