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
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 该上课时间点的默认科目。对应 ClassIsland `TimeLayoutItem.DefaultClassId`，
    /// 课表里的课程格留空时回退到这里（导入时由科目名直接填入）。
    #[serde(
        default,
        alias = "defaultClassId",
        skip_serializing_if = "Option::is_none"
    )]
    pub default_subject: Option<String>,
    /// 是否默认隐藏。对应 ClassIsland `TimeLayoutItem.IsHideDefault`：
    /// 勾上以后这个时间点只在处于它时显示，其余时候折叠掉。
    #[serde(default, alias = "isHideDefault", skip_serializing_if = "is_false")]
    pub is_hidden: bool,
}

/// `false` 不写进 YAML：这些字段绝大多数时候就是默认值，
/// 每条都写一遍会让配置文件里一半是 `null` / `false`，人肉编辑时全是噪声。
fn is_false(b: &bool) -> bool {
    !*b
}

/// `true` 不写进 YAML（用于 `is_enabled` 这类默认开启的开关）。
fn is_true(b: &bool) -> bool {
    *b
}

impl Default for TimetableSlot {
    fn default() -> Self {
        Self {
            period: None,
            start: String::new(),
            end: String::new(),
            kind: SlotKind::Class,
            name: None,
            default_subject: None,
            is_hidden: false,
        }
    }
}

impl TimetableSlot {
    /// 时长（分钟）。对应 ClassIsland `TimeLayoutItem.Last`（由结束减开始算出）。
    pub fn duration_minutes(&self) -> i64 {
        match (
            crate::time::LocalDateTime::parse_hhmm(&self.start),
            crate::time::LocalDateTime::parse_hhmm(&self.end),
        ) {
            (Some(a), Some(b)) => b as i64 - a as i64,
            _ => 0,
        }
    }
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
    #[serde(default)]
    pub id: String,
    /// 显示名称。
    #[serde(default)]
    pub name: String,
    /// 是否为当前启用项。
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_active: bool,
    /// 来源：`manual` 自主配置 / `classisland` 导入。
    #[serde(default = "default_source", skip_serializing_if = "is_manual")]
    pub source: String,
    /// 时间段列表。
    #[serde(default)]
    pub slots: Vec<TimetableSlot>,
    /// 所属分组（课表群）。对应 ClassIsland `ClassPlanGroup`；
    /// 默认 `global`，VCA 里仅用于界面上归类展示。
    #[serde(default = "default_group", skip_serializing_if = "is_global")]
    pub group: String,
}

fn default_group() -> String {
    "global".to_string()
}

/// 默认来源不写进 YAML。
fn is_manual(s: &str) -> bool {
    s == "manual"
}

/// 默认分组不写进 YAML。
fn is_global(s: &str) -> bool {
    s == "global"
}

impl Default for Timetable {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            is_active: false,
            source: default_source(),
            slots: Vec::new(),
            group: default_group(),
        }
    }
}

impl Timetable {
    /// 上课时间点（`kind = class`）按顺序列出。
    ///
    /// 课表里的课程格就是按这个顺序对齐的 —— 对应 ClassIsland
    /// `ClassPlan.ValidTimeLayoutItems`（只保留上课类型的时间点）。
    pub fn class_slots(&self) -> Vec<&TimetableSlot> {
        self.slots
            .iter()
            .filter(|s| s.kind == SlotKind::Class)
            .collect()
    }

    /// 第 `index` 个上课时间点（0 起），越界返回 None。
    pub fn class_slot(&self, index: usize) -> Option<&TimetableSlot> {
        self.class_slots().into_iter().nth(index)
    }

    /// 按开始时间找上课时间点的下标（0 起）。
    ///
    /// 旧格式的课程条目里写的是 `start: "08:00"`，新格式写的是第几节；
    /// 两边靠这个函数对上，才能把「按时间」的旧数据升级成「按时间点」的新数据。
    pub fn class_slot_index_at(&self, start: &str) -> Option<usize> {
        self.class_slots()
            .iter()
            .position(|s| s.start.trim() == start.trim())
    }

    /// 自检：时间格式、有没有上课节、有没有互相重叠的段。
    ///
    /// 单独提出来是因为这三件事在「保存时间表」与「读取时间表」两处都要报，
    /// 而它们纯属时间表自身的问题，跟课表没关系。
    pub fn issues(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.slots.is_empty() {
            return v;
        }
        if self.class_slots().is_empty() {
            v.push("时间表里没有上课节".to_string());
        }
        for (i, a) in self.slots.iter().enumerate() {
            if crate::time::LocalDateTime::parse_hhmm(&a.start).is_none()
                || crate::time::LocalDateTime::parse_hhmm(&a.end).is_none()
            {
                v.push(format!("第 {} 段时间格式错误（应为 HH:mm）", i + 1));
            }
            for (j, b) in self.slots.iter().enumerate().skip(i + 1) {
                if let (Some(s1), Some(e1), Some(s2), Some(e2)) = (
                    crate::time::LocalDateTime::parse_hhmm(&a.start),
                    crate::time::LocalDateTime::parse_hhmm(&a.end),
                    crate::time::LocalDateTime::parse_hhmm(&b.start),
                    crate::time::LocalDateTime::parse_hhmm(&b.end),
                ) {
                    if s1 < e2 && s2 < e1 {
                        v.push(format!("第 {} 段与第 {} 段时间重叠", i + 1, j + 1));
                    }
                }
            }
        }
        v
    }
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClassEntry {
    /// 星期（`Mon`..`Sun`）。作为 overrides 的 target 时可省略。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub day: String,
    /// 节次。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period: Option<u32>,
    /// 开始时间 `HH:mm`；新格式可省略，由绑定时间表补全。
    #[serde(default)]
    pub start: String,
    /// 结束时间 `HH:mm`；新格式可省略，由绑定时间表补全。
    #[serde(default)]
    pub end: String,
    /// 课程名。
    #[serde(default)]
    pub course: String,
    /// 任课教师 id。
    #[serde(default, rename = "teacherId")]
    pub teacher_id: String,
    /// 是否录制（用户自主勾选）。
    #[serde(default)]
    pub record: bool,
    /// 是否与上一节连堂合并。
    #[serde(default, skip_serializing_if = "is_false")]
    pub merge: bool,
    /// 教室（可选）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    /// 该条目自己的周次循环（`every`/`odd`/`even`）。
    /// 为空时继承所属课程表的 `cycle`；用于表达「同一星期单双周不同课」。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle: Option<String>,
}

/// 课程表。
///
/// 对应 ClassIsland `ClassPlan`。两套写法都接受：
/// - `classes`：按「星期 → 上课时间点」对齐的课程格（新的、与 ClassIsland 一致的结构）；
/// - `entries`：扁平条目（旧的、导入器产出的结构）。
///
/// [`crate::config::ScheduleFile::normalize`] 会把二者统一成 `entries`，
/// 排课逻辑只认 `entries`，所以两种写法行为完全一致。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassPlan {
    /// 课表名称（对应 ClassIsland `ClassPlan.Name`）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// 绑定的时间表 id（对应 ClassIsland `ClassPlan.TimeLayoutId`）。
    #[serde(
        default,
        alias = "timeLayoutId",
        skip_serializing_if = "String::is_empty"
    )]
    pub time_layout_id: String,
    /// 触发规则（对应 ClassIsland `ClassPlan.TimeRule`）。
    #[serde(default, alias = "timeRule")]
    pub time_rule: TimeRule,
    /// 是否默认启用（对应 ClassIsland `ClassPlan.IsEnabled`）。
    #[serde(default = "yes", alias = "isEnabled", skip_serializing_if = "is_true")]
    pub is_enabled: bool,
    /// 周次循环标记（`1`/`单周`/`双周`/`every`）。VCA 单课表时代的写法，继续保留。
    #[serde(default)]
    pub cycle: String,
    /// 课程条目。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<ClassEntry>,
    /// 按星期的课程格（对应 ClassIsland `ClassPlan.Classes`）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classes: Vec<DayClasses>,
    /// 兼容字段，同时承载两种 ClassIsland 写法：
    /// - 数字：`TimeRule.WeekDay`（1=周一 … 7=周日，0=每天）；
    /// - 字符串：旧格式里写在这一层的生效日期 `YYYY-MM-DD`。
    ///
    /// [`crate::config::ScheduleFile::normalize`] 负责把它拆进 `time_rule`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day: Option<serde_json::Value>,
}

fn yes() -> bool {
    true
}

impl Default for ClassPlan {
    fn default() -> Self {
        Self {
            name: String::new(),
            time_layout_id: String::new(),
            time_rule: TimeRule::default(),
            is_enabled: true,
            cycle: String::new(),
            entries: Vec::new(),
            classes: Vec::new(),
            day: None,
        }
    }
}

/// 周次循环（对应 ClassIsland 的多周轮换 `WeekCountDiv` / `WeekCountDivTotal`）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CycleRule {
    /// 在第几周启用：`0` 不轮换，`n` 第 n 周。
    #[serde(rename = "week", alias = "weekCountDiv", default)]
    pub week: u32,
    /// 轮换总周数：`0`/`2` 表示不轮换或双周轮换。
    #[serde(rename = "total", alias = "weekCountDivTotal", default)]
    pub total: u32,
}

impl CycleRule {
    /// 是否轮换。
    pub fn rotating(&self) -> bool {
        self.total > 1 && self.week > 0
    }

    /// 命中给定的周次序号（自 1 起）。
    ///
    /// `week_no = 0` 表示调用方**不知道**现在是第几周（VCA 只有单双周这一档
    /// 信息，见 [`crate::schedule::WeekParity::week_no`]）。
    /// 这种情况下放行：宁可照常上课，也不要因为算不出周次而静默漏课。
    pub fn matches_week(&self, week_no: u32) -> bool {
        if !self.rotating() || week_no == 0 {
            return true;
        }
        let idx = (week_no - 1) % self.total + 1;
        idx == self.week
    }
}

/// 课表触发规则（对应 ClassIsland `TimeRule`）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TimeRule {
    /// 在一周中的哪一天启用：`1` 周一 … `7` 周日；`0` 表示每天。
    #[serde(default, alias = "weekDay")]
    pub weekday: u32,
    /// 多周轮换。
    #[serde(default, alias = "weekCount")]
    pub week_count: CycleRule,
    /// 生效日期区间（VCA 扩展，ClassIsland 用临时层表达）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<crate::time::LocalDate>,
    /// 生效日期区间结束。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<crate::time::LocalDate>,
}

impl TimeRule {
    /// 规则是否命中这一天（星期 + 周次 + 日期区间）。
    pub fn matches(&self, date: crate::time::LocalDate, week_no: u32) -> bool {
        self.match_weekday(date.weekday())
            && self.from.is_none_or(|f| date >= f)
            && self.to.is_none_or(|t| date <= t)
            && self.week_count.matches_week(week_no)
    }

    /// 触发规则的星期部分是否命中。
    ///
    /// `weekday` 用 VCA 的口径（0=周一 … 6=周日），`TimeRule.weekday` 用
    /// ClassIsland 的口径（1=周一 … 7=周日，0=每天），这里做转换。
    pub fn match_weekday(&self, weekday: u32) -> bool {
        self.weekday == 0 || self.weekday == weekday + 1
    }
}

/// 某个星期下的课程格列表（对应 ClassIsland 按 `TimeRule.WeekDay` 分组后的 `Classes`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayClasses {
    /// 星期（`Mon`..`Sun`）。
    pub day: String,
    /// 课程格，按上课时间点顺序排列，索引即时间表里第几个上课节。
    #[serde(default)]
    pub slots: Vec<ClassSlot>,
}

/// 一个课程格：时间表里某个上课时间点上放什么课（对应 ClassIsland `ClassInfo`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassSlot {
    /// 对应时间表里第几个上课时间点（0 起）。
    #[serde(default)]
    pub period: usize,
    /// 科目名（对应 ClassIsland `ClassInfo.SubjectId`，VCA 直接用名字）。
    #[serde(default, alias = "subjectId")]
    pub subject: String,
    /// 任课教师 id。
    #[serde(default, alias = "teacherId")]
    pub teacher_id: String,
    /// 是否录制（VCA 自有；ClassIsland 用 `IsEnabled` 表达这节课是否启用）。
    #[serde(default = "yes", alias = "isEnabled", skip_serializing_if = "is_true")]
    pub record: bool,
    /// 是否与上一节连堂合并。
    #[serde(default, skip_serializing_if = "is_false")]
    pub merge: bool,
    /// 单双周（`every`/`odd`/`even`），空表示跟课表。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle: Option<String>,
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
