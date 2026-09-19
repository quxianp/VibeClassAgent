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
    pub fn portable() -> Self {
        let base = program_dir();
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
