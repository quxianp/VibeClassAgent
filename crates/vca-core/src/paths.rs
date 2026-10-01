//! 数据与配置目录解析、文件命名规则。
//!
//! 对应策划书 8.1 / 8.2。
//!
//! 存档的写入见 [`crate::import::save_archive`] 与 [`crate::import::write_current`]。
//! 目录的 ACL 设置需由部署方按需配置（本模块只负责路径拼装）。
//!
//! # 数据放哪里（重要）
//!
//! **默认不碰 C 盘用户目录**。课堂一体机常年只有一个系统盘，而
//! `C:\Users\<用户名>\AppData\...` 这种位置有三个实际麻烦：
//! 换个 Windows 账户就找不到了、重装系统全丢、而录像文件动辄几百 MB
//! 也不该往系统盘塞。
//!
//! 解析顺序（[`Layout::resolve`]）：
//!
//! 1. 命令行 `--data-dir` / `--config-dir`（最高优先级，便于脚本化部署）
//! 2. 环境变量 `VCA_DATA_DIR` / `VCA_CONFIG_DIR`
//! 3. **引导文件** `vca.paths.json`（放在 exe 同级，记录用户自己选的位置）
//! 4. **便携布局**：exe 同级的 `data/` 与 `config/`（只要那个目录可写就用它）
//! 5. 实在不行才退到 `%LOCALAPPDATA%`，并且会明确告知用户
//!
//! 引导文件是「让用户自己选」的落点：它必须放在 exe 旁边，
//! 因为配置目录本身就是被选择的对象 —— 不能把选择记在待选择的目录里。

use std::path::{Path, PathBuf};

use crate::DEFAULT_DATA_DIR_NAME;

/// 引导文件名（放在 exe 同级）。
pub const BOOTSTRAP_FILE: &str = "vca.paths.json";

/// 用户在引导文件里记录的选择。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PathChoice {
    /// 数据根目录。
    #[serde(default)]
    pub data_root: String,
    /// 配置根目录。
    #[serde(default)]
    pub config_root: String,
}

/// 目录来源（用于给用户一个明确的说法）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathSource {
    /// 命令行指定。
    Cli,
    /// 环境变量指定。
    Env,
    /// 引导文件（用户自己选的）。
    Bootstrap,
    /// 便携布局（exe 同级）。
    Portable,
    /// 系统用户目录兜底。
    UserFallback,
}

impl PathSource {
    /// 人类可读的说明。
    pub fn describe(self) -> &'static str {
        match self {
            Self::Cli => "命令行指定",
            Self::Env => "环境变量指定",
            Self::Bootstrap => "你之前选择的位置",
            Self::Portable => "程序所在目录（便携模式）",
            Self::UserFallback => "系统用户目录（程序目录不可写，已兜底）",
        }
    }
}

/// 程序所在目录；拿不到时退回当前工作目录。
pub fn program_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 引导文件路径。
pub fn bootstrap_path() -> PathBuf {
    program_dir().join(BOOTSTRAP_FILE)
}

/// 读取引导文件（不存在或格式不对都返回 None，不报错）。
pub fn load_bootstrap() -> Option<PathChoice> {
    let text = std::fs::read_to_string(bootstrap_path()).ok()?;
    let c: PathChoice = serde_json::from_str(&text).ok()?;
    if c.data_root.trim().is_empty() && c.config_root.trim().is_empty() {
        return None;
    }
    Some(c)
}

/// 写入引导文件（记住用户的选择）。
pub fn save_bootstrap(choice: &PathChoice) -> std::io::Result<PathBuf> {
    let p = bootstrap_path();
    let text = serde_json::to_string_pretty(choice).unwrap_or_default();
    std::fs::write(&p, text)?;
    Ok(p)
}

/// 目录是否可写（真的建一个临时文件试一下，不看 ACL 位）。
pub fn is_writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".vca_write_probe");
    match std::fs::write(&probe, b"1") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// 便携布局的基准目录。
///
/// 正常情况就是程序所在目录；但开发形态（`cargo run`）下 exe 在
/// `target/debug` 或 `target/release`，数据放那儿既会被 `cargo clean` 清掉，
/// 又让用户在仓库根目录下找不到，所以要上溯到仓库根。
fn portable_base() -> PathBuf {
    let exe_dir = program_dir();
    let name = exe_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let parent_name = exe_dir
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    if (name == "debug" || name == "release") && parent_name == "target" {
        if let Some(root) = exe_dir.parent().and_then(|p| p.parent()) {
            return root.to_path_buf();
        }
    }
    exe_dir
}

/// 目录布局解析器。
#[derive(Debug, Clone)]
pub struct Layout {
    /// 数据根目录。
    pub data_root: PathBuf,
    /// 配置根目录。
    pub config_root: PathBuf,
}

impl Layout {
    /// 以显式路径构造（便于测试与自定义部署）。
    pub fn new(data_root: impl Into<PathBuf>, config_root: impl Into<PathBuf>) -> Self {
        Self {
            data_root: data_root.into(),
            config_root: config_root.into(),
        }
    }

    /// **便携布局**：程序所在目录下的 `data/` 与 `config/`。
    ///
    /// 这是默认选择 —— 整个程序连同数据都在一个文件夹里，
    /// 拷走就能换机器，不碰系统盘。
    ///
    /// 开发形态要特殊处理：`cargo run` 时 exe 在 `target/debug/`，
    /// 照字面把数据放进那里有两个坏处 —— `cargo clean` 一跑全没了，
    /// 而且用户在自己的仓库根目录下根本找不到它们。所以识别出
    /// 「exe 位于 `target/{debug,release}`」时，基准目录上溯到仓库根。
    pub fn portable() -> Self {
        let base = portable_base();
        Self {
            data_root: base.join("data"),
            config_root: base.join("config"),
        }
    }

    /// 用户目录兜底（仅在程序目录不可写时使用）。
    pub fn user_fallback() -> Self {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("ProgramData")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("."))
            })
            .join(DEFAULT_DATA_DIR_NAME);
        Self {
            data_root: base.join("data"),
            config_root: base.join("config"),
        }
    }

    /// 按优先级解析实际使用的布局，并说明来源。
    ///
    /// 见模块文档里的顺序说明。
    pub fn resolve() -> (Self, PathSource) {
        // 3) 引导文件（用户自己选的，优先级高于任何默认值）
        if let Some(c) = load_bootstrap() {
            let data = PathBuf::from(c.data_root.trim());
            let config = if c.config_root.trim().is_empty() {
                data.join("config")
            } else {
                PathBuf::from(c.config_root.trim())
            };
            if !c.data_root.trim().is_empty() {
                return (Self::new(data, config), PathSource::Bootstrap);
            }
        }

        // 4) 便携布局：程序目录可写就用它
        let portable = Self::portable();
        if is_writable(&portable.data_root) && is_writable(&portable.config_root) {
            return (portable, PathSource::Portable);
        }

        // 5) 兜底
        (Self::user_fallback(), PathSource::UserFallback)
    }

    /// 默认布局（兼容旧调用；等价于 [`Layout::resolve`] 的结果）。
    pub fn default_windows() -> Self {
        Self::resolve().0
    }

    /// 指定 profile 的数据目录：`data/profiles/{profile}/`。
    ///
    /// `profile` 必须已通过 [`validate_profile_name`]；这是内部目录拼接，
    /// 不做二次校验（调用方在入口处统一把关，避免每条路径各写一套规则）。
    pub fn profile_data_dir(&self, profile: &str) -> PathBuf {
        self.data_root.join("profiles").join(profile)
    }

    /// 指定 profile 的配置目录：`config/profiles/{profile}/`。
    pub fn profile_config_dir(&self, profile: &str) -> PathBuf {
        self.config_root.join("profiles").join(profile)
    }

    /// 录像目录。
    pub fn raw_dir(&self, profile: &str, date: &str) -> PathBuf {
        self.profile_data_dir(profile).join("raw").join(date)
    }

    /// 音频目录。
    pub fn audio_dir(&self, profile: &str, date: &str) -> PathBuf {
        self.profile_data_dir(profile).join("audio").join(date)
    }

    /// 截图目录。
    pub fn screenshots_dir(&self, profile: &str, date: &str) -> PathBuf {
        self.profile_data_dir(profile)
            .join("screenshots")
            .join(date)
    }

    /// 转写文本目录。
    pub fn transcript_dir(&self, profile: &str, date: &str) -> PathBuf {
        self.profile_data_dir(profile).join("transcript").join(date)
    }

    /// 文档输出目录。
    pub fn docs_dir(&self, profile: &str, date: &str) -> PathBuf {
        self.profile_data_dir(profile).join("docs").join(date)
    }

    /// 软删除区（72 小时保留期）。
    pub fn trash_dir(&self, profile: &str) -> PathBuf {
        self.profile_data_dir(profile).join("trash")
    }

    /// 作业目录。
    pub fn job_dir(&self, profile: &str, job_id: &str) -> PathBuf {
        self.profile_data_dir(profile).join("jobs").join(job_id)
    }

    /// 时间表存档目录（全量版本化，不按学期归档）。
    pub fn timetable_archive_dir(&self) -> PathBuf {
        self.config_root.join("timetable").join("archive")
    }

    /// 课表存档目录（全量版本化，不按学期归档）。
    pub fn schedule_archive_dir(&self) -> PathBuf {
        self.config_root.join("schedule").join("archive")
    }
}

/// 录像文件名：`{yyyyMMdd}_{HHmm}-{HHmm}_{课程}_{profile}.mp4`（策划书 8.2）。
pub fn recording_file_name(
    date: &str,
    start: &str,
    end: &str,
    course: &str,
    profile: &str,
) -> String {
    format!("{date}_{start}-{end}_{course}_{profile}.mp4")
}

/// 时间表存档文件名：`timetable_{yyyy-ww}_{v}.yaml`。
pub fn timetable_archive_name(week: &str, version: u32) -> String {
    format!("timetable_{week}_{version}.yaml")
}

/// 课表存档文件名：`schedule_{yyyy-ww}_{v}.yaml`。
pub fn schedule_archive_name(week: &str, version: u32) -> String {
    format!("schedule_{week}_{version}.yaml")
}

/// 判断路径是否位于给定的软删除区内（用于安全校验）。
pub fn is_in_trash(path: &Path, trash: &Path) -> bool {
    path.starts_with(trash)
}

/// profile 名非法时返回的错误说明。
pub const PROFILE_RULE: &str =
    "profile 只能用字母、数字、下划线、短横线和点（如 default / teacher-1），且不能是 . 或 ..";

/// Windows 保留设备名（大小写不敏感）。用它们当目录名会失败或行为诡异。
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// 校验 profile 名是否可安全用作目录名。
///
/// # 为什么必须有这道关
///
/// profile 会被直接拼进 `data/profiles/<profile>/...` 与
/// `config/profiles/<profile>/...`。它来自命令行 `--profile` 与配置，
/// 两者都是外部输入：一个 `..` 就能把作业目录写到 data 根之外，
/// 一个 `C:\` 就能覆盖任意可写路径下的 `job.json`。
///
/// 多教师共用正是这个功能的卖点，所以这层隔离必须真的成立 ——
/// 宁可拒绝一个奇怪的名字，也不要放它进文件系统。
///
/// 允许的字符刻意收得很紧（ASCII 字母数字 + `_` `-` `.`）：
/// profile 是目录名而不是显示名，中文显示名请另设字段。
pub fn validate_profile_name(profile: &str) -> Result<&str, String> {
    let p = profile.trim();

    if p.is_empty() {
        return Err("profile 不能为空".to_string());
    }
    // 只有 "." / ".." 是纯粹的路径穿越；"." 开头的普通名字（如 .hidden）也一并拒绝，
    // 因为它们在 Windows 资源管理器里会被隐藏，用户排障时看不见。
    if p == "." || p == ".." || p.starts_with('.') {
        return Err(format!("profile「{p}」不能以点开头，{PROFILE_RULE}"));
    }
    if p.len() > 64 {
        return Err(format!("profile 太长（{} 字符，上限 64）", p.len()));
    }
    // 必须逐个字符白名单：只挡 `..` 挡不住 `a/b`、`a\b`、`C:` 这类写法
    if let Some(bad) = p
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '.'))
    {
        return Err(format!(
            "profile「{p}」含不允许的字符「{bad}」，{PROFILE_RULE}"
        ));
    }
    // 含 "." 时额外拒绝 ".." 出现在中间的写法（上面已挡纯 ..，这里防 a..b 之外的分段穿越）
    if p.contains("..") {
        return Err(format!(
            "profile「{p}」不能包含连续的「..」，{PROFILE_RULE}"
        ));
    }
    // Windows 保留名：即使带扩展名（CON.txt）也是保留的，所以按第一个 "." 之前的部分判断
    let stem = p.split('.').next().unwrap_or(p);
    if WINDOWS_RESERVED
        .iter()
        .any(|r| r.eq_ignore_ascii_case(stem))
    {
        return Err(format!(
            "profile「{p}」是 Windows 保留设备名（如 CON/NUL/COM1），换一个名字"
        ));
    }
    // 结尾是点或空格时 Windows 会静默吃掉，导致"我明明建了目录却找不到"
    if p.ends_with('.') || p.ends_with(' ') {
        return Err(format!("profile「{p}」不能以点或空格结尾"));
    }

    Ok(p)
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    #[test]
    fn accepts_ordinary_names() {
        for ok in ["default", "teacher1", "teacher-1", "class_A", "a.b"] {
            assert!(validate_profile_name(ok).is_ok(), "{ok} 应该被接受");
        }
    }

    #[test]
    fn rejects_traversal_and_separators() {
        // 这几条是本次评审的重点：它们都能把路径写出 data 根之外
        for bad in [
            "..",
            ".",
            "../../etc",
            "..\\..\\windows",
            "a/b",
            "a\\b",
            "C:\\tmp",
            "..hidden",
            "a..b",
            ".hidden",
        ] {
            assert!(validate_profile_name(bad).is_err(), "{bad} 应该被拒绝");
        }
    }

    #[test]
    fn rejects_windows_reserved_and_odd_edges() {
        for bad in ["CON", "nul", "COM1", "con.txt", "trailing.", "has space"] {
            assert!(validate_profile_name(bad).is_err(), "{bad} 应该被拒绝");
        }
        assert!(validate_profile_name("").is_err());
        assert!(validate_profile_name(&"x".repeat(65)).is_err());
    }

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(validate_profile_name("  default  ").unwrap(), "default");
    }
}
