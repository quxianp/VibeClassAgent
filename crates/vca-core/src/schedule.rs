//! 排课计划计算与悬浮窗时序（纯逻辑，无 IO，可完整单测）。
//!
//! 职责：
//! 1. 由「时间表 + 课程表 + 日期」算出当天的实际课程实例（[`LessonInstance`]）；
//! 2. 处理单双周、周末模板、临时停课；
//! 3. 连堂合并（相邻同课程同教师默认合并为一次课）；
//! 4. 计算悬浮窗显示时段（[`OverlayWindow`]）。

use crate::model::{ClassEntry, ClassPlan, Override, Timetable};
use crate::time::{LocalDate, LocalDateTime};

/// 一天中的一节课（已解析、已合并）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LessonInstance {
    /// 日期。
    pub date: LocalDate,
    /// 节次（可选）。
    pub period: Option<u32>,
    /// 开始时刻。
    pub start: LocalDateTime,
    /// 结束时刻。
    pub end: LocalDateTime,
    /// 课程名。
    pub course: String,
    /// 任课教师 id。
    pub teacher_id: String,
    /// 是否录制。
    pub record: bool,
    /// 教室。
    pub room: Option<String>,
    /// 连堂分组号：同一次合并出的课共享同一个值。
    pub merge_group: Option<u32>,
}

impl LessonInstance {
    /// 时长（分钟）。
    pub fn duration_minutes(&self) -> i64 {
        self.end.diff_minutes(self.start)
    }
}

/// 周次奇偶。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeekParity {
    /// 每周（不区分单双周）。
    Every,
    /// 单周。
    Odd,
    /// 双周。
    Even,
}

impl WeekParity {
    /// 由字符串解析，兼容 `1`/`单周`/`odd` 等写法。
    pub fn parse(s: &str) -> Self {
        let t = s.trim();
        match t {
            "1" | "单周" | "odd" | "Odd" | "ODD" => Self::Odd,
            "2" | "双周" | "even" | "Even" | "EVEN" => Self::Even,
            _ => Self::Every,
        }
    }

    /// 课程表条目的循环标记是否匹配给定周次。
    fn matches(self, entry_cycle: &str) -> bool {
        match WeekParity::parse(entry_cycle) {
            WeekParity::Every => true,
            other => other == self,
        }
    }
}

/// 星期名转 0=周一  6=周日。
pub fn weekday_from_name(name: &str) -> Option<u32> {
    match name.trim() {
        "Mon" | "mon" | "周一" | "星期一" => Some(0),
        "Tue" | "tue" | "周二" | "星期二" => Some(1),
        "Wed" | "wed" | "周三" | "星期三" => Some(2),
        "Thu" | "thu" | "周四" | "星期四" => Some(3),
        "Fri" | "fri" | "周五" | "星期五" => Some(4),
        "Sat" | "sat" | "周六" | "星期六" => Some(5),
        "Sun" | "sun" | "周日" | "星期日" | "星期天" | "周天" => Some(6),
        _ => None,
    }
}

/// 计算某一天的实际课程列表。
///
/// - `date`：目标日期；
/// - `parity`：该日期所在周是单周还是双周；
/// - `plan`：课程表（含周末模板的条目由调用方合并传入）；
/// - `overrides`：临时停课/调课；
/// - `merge_consecutive`：是否合并连堂。
pub fn plan_for_date(
    date: LocalDate,
    parity: WeekParity,
    plan: &ClassPlan,
    overrides: &[Override],
    merge_consecutive: bool,
) -> Vec<LessonInstance> {
    let wd = date.weekday();
    let mut lessons: Vec<LessonInstance> = Vec::new();

    for entry in &plan.entries {
        if let Some(day) = weekday_from_name(&entry.day) {
            if day != wd {
                continue;
            }
        } else {
            continue;
        }
        // 条目自带 cycle 时以条目为准（支持同一天单双周不同课）
        let cyc = entry.cycle.as_deref().unwrap_or(plan.cycle.as_str());
        if !parity.matches(cyc) {
            continue;
        }
        if let Some(l) = entry_to_lesson(date, entry) {
            lessons.push(l);
        }
    }

    // 临时停课 / 调课
    for ov in overrides {
        if ov.date != date {
            continue;
        }
        match ov.action.as_str() {
            "cancel" => lessons.clear(),
            "cancel-one" => {
                if let Some(t) = &ov.target {
                    lessons.retain(|l| !(l.course == t.course && l.teacher_id == t.teacher_id));
                }
            }
            "add" | "move" => {
                if let Some(t) = &ov.target {
                    if let Some(l) = entry_to_lesson(date, t) {
                        lessons.push(l);
                    }
                }
            }
            _ => {}
        }
    }

    lessons.sort_by_key(|l| (l.start.to_minutes(), l.end.to_minutes()));
    if merge_consecutive {
        merge_lessons(&mut lessons);
    }
    lessons
}

fn entry_to_lesson(date: LocalDate, entry: &ClassEntry) -> Option<LessonInstance> {
    let s = LocalDateTime::parse_hhmm(&entry.start)?;
    let e = LocalDateTime::parse_hhmm(&entry.end)?;
    if e <= s {
        return None;
    }
    Some(LessonInstance {
        date,
        period: entry.period,
        start: LocalDateTime::from_minutes(date, s as i64),
        end: LocalDateTime::from_minutes(date, e as i64),
        course: entry.course.clone(),
        teacher_id: entry.teacher_id.clone(),
        record: entry.record,
        room: entry.room.clone(),
        merge_group: None,
    })
}

/// 连堂合并：相邻、同课程、同教师、首尾相接（或间隔 <= 5 分钟）时合并为一节。
fn merge_lessons(lessons: &mut Vec<LessonInstance>) {
    if lessons.len() < 2 {
        return;
    }
    let mut out: Vec<LessonInstance> = Vec::with_capacity(lessons.len());
    let mut group: u32 = 0;

    for lesson in lessons.drain(..) {
        match out.last_mut() {
            Some(prev)
                if prev.course == lesson.course
                    && prev.teacher_id == lesson.teacher_id
                    && lesson.start.diff_minutes(prev.end) <= 5
                    && lesson.start.diff_minutes(prev.end) >= 0 =>
            {
                prev.end = lesson.end;
                prev.record = prev.record || lesson.record;
                if prev.merge_group.is_none() {
                    group += 1;
                    prev.merge_group = Some(group);
                }
            }
            _ => out.push(lesson),
        }
    }
    *lessons = out;
}

/// 悬浮窗显示时段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayWindow {
    /// 显示时刻（某节课结束 + 延迟）。
    pub show_at: LocalDateTime,
    /// 关闭时刻（下一节课开始 - 提前量）。
    pub hide_at: LocalDateTime,
}

/// 悬浮窗时序参数。
#[derive(Debug, Clone, Copy)]
pub struct OverlayTiming {
    /// 下课后多久显示（分钟）。
    pub show_delay_minutes: i64,
    /// 上课前多久关闭（分钟）。
    pub hide_before_minutes: i64,
    /// 当天无后续课程时的兜底显示时长（分钟）。
    pub fallback_minutes: i64,
}

impl Default for OverlayTiming {
    fn default() -> Self {
        Self {
            show_delay_minutes: 5,
            hide_before_minutes: 2,
            fallback_minutes: 30,
        }
    }
}

/// 由当天课程列表计算悬浮窗时段。
///
/// 规则（对应需求）：
/// - 每节课结束后 `show_delay_minutes` 分钟显示；
/// - 下一节课开始前 `hide_before_minutes` 分钟关闭；
/// - 若两节课间隔太短导致「关闭时刻 <= 显示时刻」，则该次**不显示**（避免闪现）；
/// - 当天没有后续课程时，显示 `fallback_minutes` 分钟后自动关闭。
pub fn overlay_windows(lessons: &[LessonInstance], timing: OverlayTiming) -> Vec<OverlayWindow> {
    let mut out = Vec::new();
    for (i, lesson) in lessons.iter().enumerate() {
        let show_at = lesson.end.add_minutes(timing.show_delay_minutes);

        let next_start = lessons[i + 1..]
            .iter()
            .map(|l| l.start)
            .filter(|s| *s > lesson.end)
            .min();

        let hide_at = match next_start {
            Some(ns) => ns.add_minutes(-timing.hide_before_minutes),
            None => show_at.add_minutes(timing.fallback_minutes),
        };

        if hide_at > show_at {
            out.push(OverlayWindow { show_at, hide_at });
        }
    }
    out
}

/// 判断某一时刻是否应显示悬浮窗。
pub fn overlay_visible_at(windows: &[OverlayWindow], now: LocalDateTime) -> bool {
    windows.iter().any(|w| now >= w.show_at && now < w.hide_at)
}

/// 从悬浮窗时段推算：下一次状态变化在何时（用于精确定时，避免轮询）。
pub fn next_transition(windows: &[OverlayWindow], now: LocalDateTime) -> Option<LocalDateTime> {
    let mut best: Option<LocalDateTime> = None;
    for w in windows {
        for t in [w.show_at, w.hide_at] {
            if t > now && best.map(|b| t < b).unwrap_or(true) {
                best = Some(t);
            }
        }
    }
    best
}

/// 两条课表的单双周是否恰好互斥（一条单周、一条双周）。
///
/// 互斥的两条不会同时生效，排在同一时段是正常用法，不算冲突。
fn parity_exclusive(a: Option<&str>, b: Option<&str>) -> bool {
    let (pa, pb) = (
        WeekParity::parse(a.unwrap_or("")),
        WeekParity::parse(b.unwrap_or("")),
    );
    matches!(
        (pa, pb),
        (WeekParity::Odd, WeekParity::Even) | (WeekParity::Even, WeekParity::Odd)
    )
}

/// 判断时间表与课程表是否自洽（用于导入预校验，返回问题列表）。
pub fn validate(timetable: Option<&Timetable>, plan: &ClassPlan) -> Vec<String> {
    let mut issues = Vec::new();

    if plan.entries.is_empty() {
        issues.push("课程表为空".to_string());
    }
    for (i, e) in plan.entries.iter().enumerate() {
        let idx = i + 1;
        if weekday_from_name(&e.day).is_none() {
            issues.push(format!("第 {idx} 条：无法识别的星期 `{}`", e.day));
        }
        match (
            LocalDateTime::parse_hhmm(&e.start),
            LocalDateTime::parse_hhmm(&e.end),
        ) {
            (Some(s), Some(en)) if en > s => {}
            (Some(_), Some(_)) => issues.push(format!("第 {idx} 条：结束时间不晚于开始时间")),
            _ => issues.push(format!("第 {idx} 条：时间格式错误（应为 HH:mm）")),
        }
        if e.course.trim().is_empty() {
            issues.push(format!("第 {idx} 条：课程名为空"));
        }
        if e.teacher_id.trim().is_empty() {
            issues.push(format!("第 {idx} 条：未指定教师"));
        }
    }

    // 同一天时间重叠检测
    for i in 0..plan.entries.len() {
        for j in (i + 1)..plan.entries.len() {
            let (a, b) = (&plan.entries[i], &plan.entries[j]);
            if a.day != b.day {
                continue;
            }
            if let (Some(s1), Some(e1), Some(s2), Some(e2)) = (
                LocalDateTime::parse_hhmm(&a.start),
                LocalDateTime::parse_hhmm(&a.end),
                LocalDateTime::parse_hhmm(&b.start),
                LocalDateTime::parse_hhmm(&b.end),
            ) {
                if s1 < e2 && s2 < e1 {
                    // 单双周互斥：同一天同一时段排「单周 A / 双周 B」是学校里
                    // 的常规做法，它们不会同时生效，**不是冲突**。
                    // 不特判的话，用户每次启动都会看到一条莫名其妙的
                    // 「时间重叠」，然后去改本来完全正确的课表。
                    if !parity_exclusive(a.cycle.as_deref(), b.cycle.as_deref()) {
                        issues.push(format!(
                            "第 {} 条与第 {} 条在 {} 时间重叠",
                            i + 1,
                            j + 1,
                            a.day
                        ));
                    }
                }
            }
        }
    }

    if let Some(tt) = timetable {
        if tt.slots.is_empty() {
            issues.push("时间表为空".to_string());
        }
        for (i, s) in tt.slots.iter().enumerate() {
            if LocalDateTime::parse_hhmm(&s.start).is_none()
                || LocalDateTime::parse_hhmm(&s.end).is_none()
            {
                issues.push(format!("时间表第 {} 段：时间格式错误", i + 1));
            }
        }
    }

    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SlotKind;

    fn entry(day: &str, s: &str, e: &str, course: &str, teacher: &str, record: bool) -> ClassEntry {
        ClassEntry {
            day: day.to_string(),
            period: None,
            start: s.to_string(),
            end: e.to_string(),
            course: course.to_string(),
            teacher_id: teacher.to_string(),
            record,
            merge: false,
            room: None,
            cycle: None,
        }
    }

    fn plan(entries: Vec<ClassEntry>) -> ClassPlan {
        ClassPlan {
            cycle: "every".to_string(),
            entries,
        }
    }

    #[test]
    fn filters_by_weekday() {
        let p = plan(vec![
            entry("Mon", "08:00", "08:45", "数学", "t1", true),
            entry("Tue", "09:00", "09:45", "语文", "t2", true),
        ]);
        let lessons = plan_for_date(
            LocalDate::new(2025, 3, 17), // 周一
            WeekParity::Every,
            &p,
            &[],
            false,
        );
        assert_eq!(lessons.len(), 1);
        assert_eq!(lessons[0].course, "数学");
    }

    #[test]
    fn odd_even_week_filtering() {
        let p = ClassPlan {
            cycle: "odd".to_string(),
            entries: vec![entry("Mon", "08:00", "08:45", "单周课", "t1", true)],
        };
        let date = LocalDate::new(2025, 3, 17);
        assert_eq!(
            plan_for_date(date, WeekParity::Odd, &p, &[], false).len(),
            1
        );
        assert_eq!(
            plan_for_date(date, WeekParity::Even, &p, &[], false).len(),
            0
        );
    }

    #[test]
    fn consecutive_lessons_are_merged() {
        let p = plan(vec![
            entry("Mon", "08:00", "08:45", "数学", "t1", true),
            entry("Mon", "08:45", "09:30", "数学", "t1", true),
            entry("Mon", "10:00", "10:45", "语文", "t2", false),
        ]);
        let lessons = plan_for_date(
            LocalDate::new(2025, 3, 17),
            WeekParity::Every,
            &p,
            &[],
            true,
        );
        assert_eq!(lessons.len(), 2, "连堂应合并为一节");
        assert_eq!(lessons[0].course, "数学");
        assert_eq!(lessons[0].start.minute_of_day(), 8 * 60);
        assert_eq!(lessons[0].end.minute_of_day(), 9 * 60 + 30);
        assert!(lessons[0].merge_group.is_some());
        assert_eq!(lessons[1].course, "语文");
        assert!(lessons[1].merge_group.is_none());
    }

    #[test]
    fn merge_can_be_disabled() {
        let p = plan(vec![
            entry("Mon", "08:00", "08:45", "数学", "t1", true),
            entry("Mon", "08:45", "09:30", "数学", "t1", true),
        ]);
        let lessons = plan_for_date(
            LocalDate::new(2025, 3, 17),
            WeekParity::Every,
            &p,
            &[],
            false,
        );
        assert_eq!(lessons.len(), 2);
    }

    #[test]
    fn different_course_never_merges() {
        let p = plan(vec![
            entry("Mon", "08:00", "08:45", "数学", "t1", true),
            entry("Mon", "08:45", "09:30", "物理", "t1", true),
        ]);
        let lessons = plan_for_date(
            LocalDate::new(2025, 3, 17),
            WeekParity::Every,
            &p,
            &[],
            true,
        );
        assert_eq!(lessons.len(), 2);
    }

    #[test]
    fn cancel_override_clears_day() {
        let p = plan(vec![entry("Mon", "08:00", "08:45", "数学", "t1", true)]);
        let ov = vec![Override {
            date: LocalDate::new(2025, 3, 17),
            action: "cancel".to_string(),
            target: None,
        }];
        let lessons = plan_for_date(
            LocalDate::new(2025, 3, 17),
            WeekParity::Every,
            &p,
            &ov,
            false,
        );
        assert!(lessons.is_empty());
    }

    #[test]
    fn overlay_shows_5min_after_and_hides_2min_before_next() {
        let p = plan(vec![
            entry("Mon", "08:00", "08:45", "数学", "t1", false),
            entry("Mon", "09:00", "09:45", "语文", "t2", false),
        ]);
        let lessons = plan_for_date(
            LocalDate::new(2025, 3, 17),
            WeekParity::Every,
            &p,
            &[],
            false,
        );
        let windows = overlay_windows(&lessons, OverlayTiming::default());
        assert_eq!(windows.len(), 2);

        // 第 1 节 08:45 结束 -> 08:50 显示；第 2 节 09:00 开始 -> 08:58 关闭
        assert_eq!(windows[0].show_at.minute_of_day(), 8 * 60 + 50);
        assert_eq!(windows[0].hide_at.minute_of_day(), 8 * 60 + 58);

        // 最后一节 09:45 结束 -> 09:50 显示，无后续课程 -> 兜底 30 分钟
        assert_eq!(windows[1].show_at.minute_of_day(), 9 * 60 + 50);
        assert_eq!(windows[1].hide_at.minute_of_day(), 10 * 60 + 20);
    }

    #[test]
    fn overlay_skipped_when_gap_too_short() {
        // 08:45 结束 -> 08:50 显示；下一节 08:52 开始 -> 08:50 关闭 -> 无效，应跳过
        let p = plan(vec![
            entry("Mon", "08:00", "08:45", "数学", "t1", false),
            entry("Mon", "08:52", "09:30", "语文", "t2", false),
        ]);
        let lessons = plan_for_date(
            LocalDate::new(2025, 3, 17),
            WeekParity::Every,
            &p,
            &[],
            false,
        );
        let windows = overlay_windows(&lessons, OverlayTiming::default());
        // 第 1 节：08:45 结束 -> 08:50 显示；下一节 08:52 开始 -> 08:50 关闭，
        // 关闭时刻不晚于显示时刻，故该次不显示。
        // 只有第 2 节产生窗口：09:30 结束 -> 09:35 显示，无后续课程 -> 兜底 30 分钟。
        assert_eq!(windows.len(), 1, "间隔过短的那次不应显示");
        assert_eq!(windows[0].show_at.minute_of_day(), 9 * 60 + 35);
        assert_eq!(windows[0].hide_at.minute_of_day(), 10 * 60 + 5);
    }

    #[test]
    fn overlay_visible_and_next_transition() {
        let w = vec![OverlayWindow {
            show_at: LocalDateTime::new(LocalDate::new(2025, 3, 17), 8, 50),
            hide_at: LocalDateTime::new(LocalDate::new(2025, 3, 17), 8, 58),
        }];
        let d = LocalDate::new(2025, 3, 17);
        assert!(!overlay_visible_at(&w, LocalDateTime::new(d, 8, 49)));
        assert!(overlay_visible_at(&w, LocalDateTime::new(d, 8, 50)));
        assert!(overlay_visible_at(&w, LocalDateTime::new(d, 8, 57)));
        assert!(!overlay_visible_at(&w, LocalDateTime::new(d, 8, 58)));

        assert_eq!(
            next_transition(&w, LocalDateTime::new(d, 8, 40)).unwrap(),
            LocalDateTime::new(d, 8, 50)
        );
        assert_eq!(
            next_transition(&w, LocalDateTime::new(d, 8, 55)).unwrap(),
            LocalDateTime::new(d, 8, 58)
        );
        assert!(next_transition(&w, LocalDateTime::new(d, 9, 0)).is_none());
    }

    #[test]
    fn validate_reports_problems() {
        let p = plan(vec![
            entry("Funday", "08:00", "08:45", "数学", "t1", true),
            entry("Mon", "09:00", "08:00", "语文", "t2", true),
            entry("Mon", "07:00", "08:00", "英语", "", true),
        ]);
        let issues = validate(None, &p);
        assert!(issues.iter().any(|s| s.contains("星期")));
        assert!(issues.iter().any(|s| s.contains("结束时间")));
        assert!(issues.iter().any(|s| s.contains("教师")));
    }

    #[test]
    fn validate_detects_overlap() {
        let p = plan(vec![
            entry("Mon", "08:00", "09:00", "数学", "t1", true),
            entry("Mon", "08:30", "09:30", "语文", "t2", true),
        ]);
        let issues = validate(None, &p);
        assert!(issues.iter().any(|s| s.contains("重叠")));
    }

    #[test]
    fn odd_even_same_slot_is_not_a_conflict() {
        // 同一天同一时段排「单周 A / 双周 B」是常规做法，两条不会同时生效。
        // 以前会被误报成「时间重叠」，用户看到只会去改本来正确的课表。
        let mut a = entry("Mon", "08:00", "09:00", "数学", "t1", true);
        a.cycle = Some("odd".into());
        let mut b = entry("Mon", "08:30", "09:30", "语文", "t2", true);
        b.cycle = Some("even".into());
        let issues = validate(None, &plan(vec![a, b]));
        assert!(
            !issues.iter().any(|s| s.contains("重叠")),
            "单双周互斥不该算冲突：{issues:?}"
        );
    }

    #[test]
    fn same_parity_still_conflicts() {
        // 两条都是单周 —— 那才是真的撞车，必须报出来
        let mut a = entry("Mon", "08:00", "09:00", "数学", "t1", true);
        a.cycle = Some("odd".into());
        let mut b = entry("Mon", "08:30", "09:30", "语文", "t2", true);
        b.cycle = Some("odd".into());
        let issues = validate(None, &plan(vec![a, b]));
        assert!(issues.iter().any(|s| s.contains("重叠")));
    }

    #[test]
    fn timetable_slot_kind_is_used() {
        let tt = Timetable {
            id: "t".into(),
            name: "n".into(),
            is_active: true,
            source: "manual".into(),
            slots: vec![crate::model::TimetableSlot {
                period: Some(1),
                start: "08:00".into(),
                end: "08:45".into(),
                kind: SlotKind::Class,
                name: None,
            }],
        };
        assert!(validate(
            Some(&tt),
            &plan(vec![entry("Mon", "08:00", "08:45", "x", "t", true)])
        )
        .is_empty());
    }
}
