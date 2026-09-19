//! 作业状态机。
//!
//! 严格对应策划书 6.1 的流水线：
//! `RECORDED  TRANSCRIBED  EXTRACTED  LINKED  DOC_READY  PUSHED  CLEANED`
//!
//! 持久化见 [`crate::store::JobStore`]，调度见 `vca-engine` 的 pipeline 与 daemon。

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// 作业状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum JobState {
    /// 已创建，尚未开始录制。
    Pending,
    /// 已录制完成，等待后处理。
    Recorded,
    /// 已转写。
    Transcribed,
    /// 已提取要点。
    Extracted,
    /// 已关联截图。
    Linked,
    /// 文档已生成（Word/PDF）。
    DocReady,
    /// 已推送成功（**72 小时倒计时起点**）。
    Pushed,
    /// 已清理（到期删除录像）。
    Cleaned,
    /// 处理失败，待人工重试。
    Failed,
}

impl JobState {
    /// 按策划书定义判断能否迁移到 `next`。
    ///
    /// 允许失败态从任意状态进入；允许 `Failed` 重试回到 `Recorded`。
    pub fn can_transition_to(self, next: JobState) -> bool {
        use JobState::*;
        if next == Failed {
            return true;
        }
        matches!(
            (self, next),
            (Pending, Recorded)
                | (Recorded, Transcribed)
                | (Transcribed, Extracted)
                | (Extracted, Linked)
                | (Linked, DocReady)
                | (DocReady, Pushed)
                | (Pushed, Cleaned)
                | (Failed, Recorded)
        )
    }

    /// 执行一次状态迁移，非法则报错。
    pub fn transition_to(self, next: JobState) -> Result<JobState> {
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            Err(CoreError::InvalidTransition {
                from: self,
                to: next,
            })
        }
    }
}

/// 作业记录（状态机载体）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    /// 作业 id。
    pub id: String,
    /// 归属 profile。
    pub profile: String,
    /// 当前状态。
    pub state: JobState,
    /// 推送成功时刻（RFC3339）。清理倒计时的起点，未推送时为 None。
    #[serde(default)]
    pub push_succeeded_at: Option<String>,
    /// 录像到期删除时刻（RFC3339）= push_succeeded_at + 72h。
    #[serde(default)]
    pub expire_at: Option<String>,
    /// 最近一次错误信息。
    #[serde(default)]
    pub last_error: Option<String>,
    /// 课程名。
    #[serde(default)]
    pub course: String,
    /// 任课教师 id。
    #[serde(default)]
    pub teacher_id: String,
    /// 上课日期（yyyy-MM-dd）。
    #[serde(default)]
    pub date: String,
    /// 开始时刻。
    #[serde(default)]
    pub start: String,
    /// 结束时刻。
    #[serde(default)]
    pub end: String,
    /// 录像文件路径。
    #[serde(default)]
    pub video_path: Option<String>,
    /// 音频文件路径。
    #[serde(default)]
    pub audio_path: Option<String>,
    /// 截图文件列表。
    #[serde(default)]
    pub screenshots: Vec<String>,
    /// 生成的 Word 路径。
    #[serde(default)]
    pub docx_path: Option<String>,
    /// 生成的 PDF 路径。
    #[serde(default)]
    pub pdf_path: Option<String>,
}
