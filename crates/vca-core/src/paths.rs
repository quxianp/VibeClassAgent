//! 数据与配置目录解析、文件命名规则。
//!
//! 对应策划书 8.1 / 8.2。
//!
//! 存档的写入见 [`crate::import::save_archive`] 与 [`crate::import::write_current`]。
//! 目录的 ACL 设置需由部署方按需配置（本模块只负责路径拼装）。

use std::path::{Path, PathBuf};

use crate::DEFAULT_DATA_DIR_NAME;

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

    /// 默认布局：`%ProgramData%\\<数据目录名>\\` 下的 data / config。
    ///
    /// 若无法解析 ProgramData，则退回到当前工作目录下的相对路径。
    pub fn default_windows() -> Self {
        let base = std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join(DEFAULT_DATA_DIR_NAME);
        Self {
            data_root: base.join("data"),
            config_root: base.join("config"),
        }
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
