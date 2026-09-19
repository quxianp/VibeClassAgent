//! 领域模型定义。
//!
//! 严格对应策划书第 1.4 节「时间表与课程表分离」的设计：
//! - [`Timetable`] 作息骨架（第几节几点到几点、课间与休息段）
//! - [`ClassPlan`] 课程填充（某天某节是什么课、哪位老师、是否录制）
//!
//! 解析与排课在 [`crate::schedule`]，导入在 [`crate::import`]，
//! 校验见 `schedule::validate` 与 `model::OverlaySettings::validate`。

use serde::{Deserialize, Serialize};

/// 时间表中的一段（一节课或一段休息）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimetableSlot {
    /// 节次（休息段可为 None）。
    pub period: Option<u32>,
    /// 开始时间，格式 `HH:mm`。
    pub start: String,
    /// 结束时间，格式 `HH:mm`。
    pub end: String,
    /// 段类型：`class` 上课节 / `break` 休息段。
    ///
    /// 同时接受 `type:` 与 `kind:` 两种写法文档与示例长期用的是 `type`，
    /// 而 Rust 关键字 `type` 不能直接做字段名，所以用别名兼容。
    #[serde(alias = "type")]
    pub kind: SlotKind,
    /// 休息段名称（如「午餐、午休」），仅 `break` 有意义。
    #[serde(default)]
    pub name: Option<String>,
}

/// 时间段类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SlotKind {
    /// 上课节。
    Class,
    /// 休息段（课间、午休、晚餐等）。
    Break,
}

/// 时间表：一天的作息骨架。可被多份课程表复用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Timetable {
    /// 唯一标识。
    pub id: String,
    /// 显示名称。
    pub name: String,
    /// 是否为当前启用项。
    #[serde(default)]
    pub is_active: bool,
    /// 来源：`manual` 自主配置 / `classisland` 导入。
    #[serde(default = "default_source")]
    pub source: String,
    /// 时间段列表。
    #[serde(default)]
    pub slots: Vec<TimetableSlot>,
}

fn default_source() -> String {
    "manual".to_string()
}

/// 教师信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Teacher {
    /// 唯一标识。
    pub id: String,
    /// 姓名。
    pub name: String,
    /// 绑定的 profile 名（多教师共用时的数据隔离键）。
    pub profile: String,
}

/// 课程表条目：某天某节的一节课。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassEntry {
    /// 星期（`Mon`..`Sun`）。作为 overrides 的 target 时可省略。
    #[serde(default)]
    pub day: String,
    /// 节次。
    #[serde(default)]
    pub period: Option<u32>,
    /// 开始时间 `HH:mm`。
    pub start: String,
    /// 结束时间 `HH:mm`。
    pub end: String,
    /// 课程名。
    pub course: String,
    /// 任课教师 id。
    #[serde(rename = "teacherId")]
    pub teacher_id: String,
    /// 是否录制（用户自主勾选）。
    #[serde(default)]
    pub record: bool,
    /// 是否与上一节连堂合并。
    #[serde(default)]
    pub merge: bool,
    /// 教室（可选）。
    #[serde(default)]
    pub room: Option<String>,
    /// 该条目自己的周次循环（`every`/`odd`/`even`）。
    /// 为空时继承所属课程表的 `cycle`；用于表达「同一星期单双周不同课」。
    #[serde(default)]
    pub cycle: Option<String>,
}

/// 课程表。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassPlan {
    /// 周次循环标记（`1`/`单周`/`双周`/`every`）。
    #[serde(default)]
    pub cycle: String,
    /// 课程条目。
    #[serde(default)]
    pub entries: Vec<ClassEntry>,
}

/// 处理窗口：一天中的自动后处理时段（策划书 1.6）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessingWindow {
    /// 窗口名称，如「午休」。
    pub name: String,
    /// 开始时间 `HH:mm`。
    pub start: String,
    /// 结束时间 `HH:mm`。
    pub end: String,
    /// 处理范围：`morning` / `afternoon` / `evening`。
    pub scope: String,
    /// 并发上限（弱机默认 1，串行）。
    #[serde(default = "default_one")]
    pub max_concurrent: u32,
}

fn default_one() -> u32 {
    1
}

/// 已录制的一节课（一次录制产出的原始素材）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recording {
    /// 作业 id。
    pub job_id: String,
    /// 归属 profile。
    pub profile: String,
    /// 课程名。
    pub course: String,
    /// 录像文件路径。
    pub video_path: String,
    /// 音频文件路径（供转写使用）。
    #[serde(default)]
    pub audio_path: Option<String>,
    /// 截图文件路径列表。
    #[serde(default)]
    pub screenshots: Vec<String>,
    /// 开始时间（RFC3339）。
    pub started_at: String,
    /// 结束时间（RFC3339）。
    pub ended_at: String,
}

/// 转写片段。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegment {
    /// 起始秒。
    pub start: f64,
    /// 结束秒。
    pub end: f64,
    /// 文本。
    pub text: String,
}

/// 一节课的提取要点。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LessonSummary {
    /// 节次序号。
    pub index: u32,
    /// 标题。
    pub title: String,
    /// 课堂重点。
    #[serde(default)]
    pub points: Vec<String>,
    /// 重点通知。
    #[serde(default)]
    pub notices: Vec<String>,
}

/// 临时停课 / 调课记录。
///
/// 对应策划书 9.2 的 `overrides`：
/// - `cancel`：该日整天停课；
/// - `cancel-one`：仅取消指定课程；
/// - `add` / `move`：追加或调整一节课（用 `target` 描述）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Override {
    /// 目标日期。
    pub date: crate::time::LocalDate,
    /// 动作：`cancel` / `cancel-one` / `add` / `move`。
    pub action: String,
    /// 目标课程（`add` / `move` / `cancel-one` 使用）。
    #[serde(default)]
    pub target: Option<ClassEntry>,
}

/// 悬浮窗外观与时序配置（对应需求：下课后 5 分钟显示，上课前 2 分钟关闭）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlaySettings {
    /// 是否启用悬浮窗。
    pub enabled: bool,
    /// 显示文本。
    pub text: String,
    /// 下课后延迟显示（分钟），默认 5。
    pub show_delay_minutes: i64,
    /// 上课前提前关闭（分钟），默认 2。
    pub hide_before_minutes: i64,
    /// 当天无后续课程时的兜底显示时长（分钟），默认 30。
    pub fallback_minutes: i64,
    /// 距屏幕上边距（像素）。
    pub margin_top: i32,
    /// 距屏幕右边距（像素）。
    pub margin_right: i32,
    /// 窗口宽度（像素）。
    pub width: i32,
    /// 窗口高度（像素）。
    pub height: i32,
    /// 背景色，`#RRGGBB`。
    pub bg_color: String,
    /// 文字色，`#RRGGBB`。
    pub text_color: String,
    /// 字号（逻辑像素）。
    pub font_size: i32,
    /// 是否只对「勾选录制」的课显示。
    pub record_only: bool,
    /// 字体名，默认 Consolas。
    pub font_face: String,
    /// 是否启用液态玻璃背景。
    pub liquid_glass: bool,
    /// 玻璃层不透明度（0-255）。
    pub glass_alpha: u8,
    /// 是否采用 Windows 系统主机色作为玻璃色调。
    pub use_system_accent: bool,
}

impl Default for OverlaySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            text: "Agent Overrr :)".to_string(),
            show_delay_minutes: 5,
            hide_before_minutes: 2,
            fallback_minutes: 30,
            margin_top: 16,
            margin_right: 16,
            width: 240,
            height: 72,
            bg_color: "#1F6FEB".to_string(),
            text_color: "#FFFFFF".to_string(),
            font_size: 18,
            record_only: false,
            font_face: "Consolas".to_string(),
            liquid_glass: true,
            glass_alpha: 160,
            use_system_accent: true,
        }
    }
}

impl OverlaySettings {
    /// 转为纯时序参数。
    pub fn timing(&self) -> crate::schedule::OverlayTiming {
        crate::schedule::OverlayTiming {
            show_delay_minutes: self.show_delay_minutes,
            hide_before_minutes: self.hide_before_minutes,
            fallback_minutes: self.fallback_minutes,
        }
    }

    /// 校验配置，返回问题列表。
    pub fn validate(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.text.trim().is_empty() {
            v.push("悬浮窗文本为空".to_string());
        }
        if self.show_delay_minutes < 0 {
            v.push("show_delay_minutes 不能为负".to_string());
        }
        if self.hide_before_minutes < 0 {
            v.push("hide_before_minutes 不能为负".to_string());
        }
        if self.fallback_minutes <= 0 {
            v.push("fallback_minutes 必须为正".to_string());
        }
        if self.width <= 0 || self.height <= 0 {
            v.push("窗口尺寸必须为正".to_string());
        }
        for (name, c) in [
            ("bg_color", &self.bg_color),
            ("text_color", &self.text_color),
        ] {
            if parse_hex_color(c).is_none() {
                v.push(format!("{name} 不是合法的 #RRGGBB 颜色：{c}"));
            }
        }
        v
    }
}

/// 解析 `#RRGGBB` / `RRGGBB` 为 (r, g, b)。
pub fn parse_hex_color(s: &str) -> Option<(u8, u8, u8)> {
    let t = s.trim().trim_start_matches('#');
    if t.len() != 6 || !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let r = u8::from_str_radix(&t[0..2], 16).ok()?;
    let g = u8::from_str_radix(&t[2..4], 16).ok()?;
    let b = u8::from_str_radix(&t[4..6], 16).ok()?;
    Some((r, g, b))
}
