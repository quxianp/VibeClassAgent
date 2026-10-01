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
    /// 检查：接口大版本、入口非空且安全、运行模式合法。
    ///
    /// # 为什么只接受 `process`
    ///
    /// `cdylib` 与 `wasm` 在数据结构里留着，但 `runner` 只会
    /// `Command::new(entry)` 去启动一个可执行文件 —— 声明成那两种模式
    /// 会被当成 exe 执行，然后以一句看不懂的系统错误告终（评审 R-14）。
    ///
    /// 与其接受一个做不到的声明，不如在校验阶段就说清楚。等真实现了
    /// 对应加载器，再把这两个分支放回来。
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
        match self.runtime.r#type.as_str() {
            "process" => {}
            "cdylib" | "wasm" => issues.push(format!(
                "运行模式「{}」尚未实现（当前只支持 process：独立进程 + JSON-RPC over stdio）",
                self.runtime.r#type
            )),
            other => issues.push(format!("未知运行模式: {other}（可选：process）")),
        }
        // 入口必须相对且不能爬出插件目录：绝对路径与 `..` 都能指向
        // 宿主机器上的任意文件（评审 R-15）。
        if let Some(issue) = check_entry_path(&self.runtime.entry) {
            issues.push(issue);
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

/// 校验插件入口路径；不合法时返回说明。
///
/// 允许的形式只有「相对路径」：
///
/// - 绝对路径（`C:\...`、`\\server\share`、`/usr/bin/...`）会让清单指向
///   宿主机器上的任意文件；
/// - 含 `..` 的相对路径能爬出插件目录，效果一样；
/// - 含盘符前缀（`C:foo.exe`）在 Windows 上是"当前盘相对路径"，同样要挡。
///
/// 真正的边界由安装阶段决定（入口应当在解压出来的插件目录里），
/// 这里做的是**清单层面的静态拒绝**：能在不碰文件系统时挡掉的就挡掉。
fn check_entry_path(entry: &str) -> Option<String> {
    let e = entry.trim();
    if e.is_empty() {
        // 空入口已由调用方单独报错，不重复
        return None;
    }
    // Windows 与 Unix 的分隔符都要查：插件清单是跨平台格式
    let path = std::path::Path::new(e);
    if path.is_absolute() {
        return Some(format!("运行时入口「{e}」必须是相对路径，不能用绝对路径"));
    }
    if e.starts_with('/') || e.starts_with('\\') {
        return Some(format!("运行时入口「{e}」不能以路径分隔符开头"));
    }
    // 盘符前缀：C:foo
    let b = e.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        return Some(format!("运行时入口「{e}」不能带盘符"));
    }
    // 逐段检查，任何一段是 `..` 就越界
    if e.split(['/', '\\']).any(|seg| seg == "..") {
        return Some(format!("运行时入口「{e}」不能包含「..」（会爬出插件目录）"));
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;

    fn base(entry: &str, ty: &str) -> Manifest {
        Manifest {
            plugin: PluginMeta {
                id: "p.test".into(),
                name: "测试".into(),
                version: "1.0.0".into(),
                api_version: "1".into(),
                kind: vca_ipc::PluginKind::Pusher,
                author: None,
                license: None,
                risk_note: None,
            },
            runtime: RuntimeSpec {
                r#type: ty.into(),
                entry: entry.into(),
                timeout_sec: 30,
                restart: None,
            },
            permissions: Permissions::default(),
            config: serde_json::Value::Null,
        }
    }

    #[test]
    fn accepts_plain_process_entry() {
        let r = base("bin/push.exe", "process").validate("1");
        assert!(r.ok, "应通过，但报了：{:?}", r.issues);
    }

    #[test]
    fn rejects_unimplemented_runtime_types() {
        // 声明了却没实现加载器，比直接不支持更糟：会被当 exe 执行
        for ty in ["cdylib", "wasm"] {
            let r = base("bin/push.exe", ty).validate("1");
            assert!(!r.ok, "{ty} 应被拒绝");
            assert!(
                r.issues.iter().any(|i| i.contains("尚未实现")),
                "{ty} 的说明里应指出未实现，实际：{:?}",
                r.issues
            );
        }
    }

    #[test]
    fn rejects_unknown_runtime_type() {
        let r = base("bin/push.exe", "python").validate("1");
        assert!(!r.ok);
        assert!(r.issues.iter().any(|i| i.contains("未知运行模式")));
    }

    #[test]
    fn rejects_escaping_entry_paths() {
        for bad in [
            r"C:\Windows\System32\cmd.exe",
            r"\\server\share\evil.exe",
            "/usr/bin/env",
            "../outside.exe",
            r"..\outside.exe",
            r"bin\..\..\evil.exe",
            "C:evil.exe",
        ] {
            let r = base(bad, "process").validate("1");
            assert!(!r.ok, "入口 {bad} 应被拒绝");
        }
    }

    #[test]
    fn rejects_api_mismatch() {
        let r = base("bin/push.exe", "process").validate("2");
        assert!(!r.ok);
        assert!(r.issues.iter().any(|i| i.contains("大版本不匹配")));
    }

    #[test]
    fn rejects_empty_entry() {
        let r = base("   ", "process").validate("1");
        assert!(!r.ok);
        assert!(r.issues.iter().any(|i| i.contains("entry 为空")));
    }
}
