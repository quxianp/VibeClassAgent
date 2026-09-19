//! 处理窗口推导：从时间表的**休息段**自动得到「何时做课后处理」。
//!
//! 为什么这么设计：各校作息差异极大，硬编码"午休 12:10"必然不适配。
//! 时间表里已经标好了 `type: break` 的段落（午餐午休、晚餐、大课间等），
//! 直接复用它们作为处理窗口，既准确又零配置。
//!
//! 规则：
//! - 只取时长 >= `min_minutes`（默认 15 分钟）的休息段（课间 5 分钟不够转写）；
//! - `scope` 由开始时间推断：< 12:00  morning；12:00..18:00  afternoon；否则 evening；
//! - 窗口默认串行（`max_concurrent = 1`），避免在弱机上多进程抢 CPU。

use crate::model::{ProcessingWindow, SlotKind, Timetable, TimetableSlot};
use crate::time::LocalDateTime;

/// 推导参数。
#[derive(Debug, Clone, Copy)]
pub struct WindowSpec {
    /// 休息段至少多长才作为处理窗口（分钟）。
    pub min_minutes: i64,
    /// 窗口开始时间相对休息段起点的提前量（分钟，通常为 0）。
    pub lead_minutes: i64,
    /// 窗口结束时间相对休息段终点的提前量（分钟，留出准备时间）。
    pub trail_minutes: i64,
    /// 并发上限（弱机建议 1）。
    pub max_concurrent: u32,
}

impl Default for WindowSpec {
    fn default() -> Self {
        Self {
            min_minutes: 15,
            lead_minutes: 0,
            trail_minutes: 0,
            max_concurrent: 1,
        }
    }
}

/// 依据 `scope` 判断休息段属于上午 / 下午 / 晚间。
fn scope_of(start_minute: u32) -> &'static str {
    if start_minute < 12 * 60 {
        "morning"
    } else if start_minute < 18 * 60 {
        "afternoon"
    } else {
        "evening"
    }
}

/// 从时间表推导处理窗口。
///
/// **优先挑「午休」与「晚餐 / 放学后」这两类休息段** —— 这是需求里的默认安排，
/// 也正好落在 DeepSeek 的闲时里。只有当时间表里一个都没标出来时
/// （休息段没写名字、或者名字里没有这些词），才退回到「所有够长的休息段」：
/// 名字没对上就不处理，比多跑几个窗口糟糕得多。
pub fn derive_windows(timetable: &Timetable, spec: WindowSpec) -> Vec<ProcessingWindow> {
    let viable = |slot: &TimetableSlot| is_viable_break(slot, spec.min_minutes);
    let meal: Vec<&TimetableSlot> = timetable
        .slots
        .iter()
        .filter(|s| viable(s) && is_meal_break(s))
        .collect();
    let picked: Vec<&TimetableSlot> = if meal.is_empty() {
        timetable.slots.iter().filter(|s| viable(s)).collect()
    } else {
        meal
    };

    let mut out: Vec<ProcessingWindow> = Vec::new();
    for slot in picked {
        let Some(s) = LocalDateTime::parse_hhmm(&slot.start) else {
            continue;
        };
        let Some(e) = LocalDateTime::parse_hhmm(&slot.end) else {
            continue;
        };
        let start = (s as i64 + spec.lead_minutes).clamp(0, 1439);
        let end = (e as i64 - spec.trail_minutes).clamp(0, 1439);
        if end <= start {
            continue;
        }
        let name = slot
            .name
            .clone()
            .unwrap_or_else(|| format!("休息段 {}", slot.start));
        out.push(ProcessingWindow {
            name,
            start: format!("{:02}:{:02}", start / 60, start % 60),
            end: format!("{:02}:{:02}", end / 60, end % 60),
            scope: scope_of(s).to_string(),
            max_concurrent: spec.max_concurrent,
        });
    }
    out.sort_by(|a, b| a.start.cmp(&b.start));
    out
}

/// 判断某一时刻是否落在某个处理窗口内；返回窗口名。
pub fn window_at(windows: &[ProcessingWindow], now_minute: u32) -> Option<&ProcessingWindow> {
    windows.iter().find(|w| {
        match (
            LocalDateTime::parse_hhmm(&w.start),
            LocalDateTime::parse_hhmm(&w.end),
        ) {
            (Some(s), Some(e)) => now_minute >= s && now_minute < e,
            _ => false,
        }
    })
}

/// 依据课程时段推断它属于哪个 scope（用于把作业分派到对应窗口）。
pub fn scope_for_lesson(start_minute: u32) -> &'static str {
    scope_of(start_minute)
}

/// 校验一组窗口：返回问题列表。
pub fn validate_windows(windows: &[ProcessingWindow]) -> Vec<String> {
    let mut issues = Vec::new();
    for w in windows.iter() {
        match (
            LocalDateTime::parse_hhmm(&w.start),
            LocalDateTime::parse_hhmm(&w.end),
        ) {
            (Some(s), Some(e)) if e > s => {}
            (Some(_), Some(_)) => issues.push(format!("窗口「{}」结束不晚于开始", w.name)),
            _ => issues.push(format!("窗口「{}」时间格式错误", w.name)),
        }
        if w.max_concurrent == 0 {
            issues.push(format!("窗口「{}」max_concurrent 必须为正", w.name));
        }
    }
    // 重叠检测
    for i in 0..windows.len() {
        for j in (i + 1)..windows.len() {
            if let (Some(s1), Some(e1), Some(s2), Some(e2)) = (
                LocalDateTime::parse_hhmm(&windows[i].start),
                LocalDateTime::parse_hhmm(&windows[i].end),
                LocalDateTime::parse_hhmm(&windows[j].start),
                LocalDateTime::parse_hhmm(&windows[j].end),
            ) {
                if s1 < e2 && s2 < e1 {
                    issues.push(format!(
                        "窗口「{}」与「{}」时间重叠",
                        windows[i].name, windows[j].name
                    ));
                }
            }
        }
    }
    issues
}

/// 示例：判断一个休息段是否值得作为处理窗口。
pub fn is_viable_break(slot: &TimetableSlot, min_minutes: i64) -> bool {
    if slot.kind != SlotKind::Break {
        return false;
    }
    match (
        LocalDateTime::parse_hhmm(&slot.start),
        LocalDateTime::parse_hhmm(&slot.end),
    ) {
        (Some(s), Some(e)) => (e as i64 - s as i64) >= min_minutes,
        _ => false,
    }
}

/// 名字里出现这些词，就认为它是「午休」。
const LUNCH_HINTS: &[&str] = &["午休", "午餐", "午饭", "中午", "午间", "lunch", "noon"];

/// 「晚餐 / 放学后」。
const DINNER_HINTS: &[&str] = &[
    "晚餐",
    "晚饭",
    "放学",
    "晚自习",
    "晚间",
    "dinner",
    "evening",
    "after school",
];

/// 这个休息段是不是「午休」或「晚餐 / 放学后」。
///
/// 按名字判断是有意为之：`type: break` 只说明「不上课」，说明不了
/// 「这段够不够安静、够不够长用来做课后处理」—— 那恰恰是午休与晚餐的特征。
/// 名字没写时返回 false，由 [`derive_windows`] 统一走兜底路径。
pub fn is_meal_break(slot: &TimetableSlot) -> bool {
    match &slot.name {
        Some(n) => {
            let n = n.to_lowercase();
            LUNCH_HINTS
                .iter()
                .chain(DINNER_HINTS.iter())
                .any(|k| n.contains(k))
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_lunch_and_dinner_breaks() {
        let t = tt(vec![
            class(1, "08:00", "08:45"),
            brk("08:45", "09:00", "课间"),
            class(2, "09:00", "09:45"),
            brk("12:00", "14:00", "午餐午休"),
            class(3, "14:00", "14:45"),
            brk("18:00", "19:00", "晚餐"),
        ]);
        let ws = derive_windows(&t, WindowSpec::default());
        assert_eq!(ws.len(), 2, "只该挑出午休与晚餐，课间不参与");
        assert_eq!(ws[0].name, "午餐午休");
        assert_eq!(ws[1].name, "晚餐");
        assert_eq!(ws[1].scope, "evening");
    }

    #[test]
    fn meal_break_needs_a_name() {
        let mut s = brk("12:00", "14:00", "");
        assert!(!is_meal_break(&s), "空名字不算");
        s.name = None;
        assert!(!is_meal_break(&s), "没名字不算");
    }

    fn tt(slots: Vec<TimetableSlot>) -> Timetable {
        Timetable {
            id: "t".into(),
            name: "n".into(),
            is_active: true,
            source: "manual".into(),
            slots,
        }
    }

    fn class(p: u32, s: &str, e: &str) -> TimetableSlot {
        TimetableSlot {
            period: Some(p),
            start: s.into(),
            end: e.into(),
            kind: SlotKind::Class,
            name: None,
        }
    }

    fn brk(s: &str, e: &str, name: &str) -> TimetableSlot {
        TimetableSlot {
            period: None,
            start: s.into(),
            end: e.into(),
            kind: SlotKind::Break,
            name: Some(name.into()),
        }
    }

    #[test]
    fn derives_windows_from_breaks() {
        let t = tt(vec![
            class(1, "08:00", "08:45"),
            brk("09:40", "10:00", "大课间"), // 20 分钟：够长，但不是午休/晚餐
            class(2, "10:00", "10:45"),
            brk("12:10", "13:30", "午餐午休"), // 80 分钟 -> 保留（首选）
            class(3, "14:00", "14:45"),
            brk("17:00", "17:05", "小课间"), // 5 分钟 -> 丢弃
            class(4, "19:00", "19:40"),
            brk("20:30", "21:30", "晚餐"), // 60 分钟 -> 保留（首选）
        ]);
        let w = derive_windows(&t, WindowSpec::default());
        assert_eq!(
            w.len(),
            2,
            "只挑午休与晚餐：大课间够长，但不是做课后处理的时段"
        );
        assert_eq!(w[0].name, "午餐午休");
        assert_eq!(w[0].scope, "afternoon");
        assert_eq!(w[1].name, "晚餐");
        assert_eq!(w[1].scope, "evening");
        assert_eq!(w[1].max_concurrent, 1);
    }

    #[test]
    fn fallback_keeps_long_breaks_when_unlabeled() {
        let t = tt(vec![
            class(1, "08:00", "08:45"),
            brk("09:40", "10:00", "大课间"),
            brk("17:00", "17:05", "小课间"),
        ]);
        let w = derive_windows(&t, WindowSpec::default());
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].name, "大课间");
    }

    #[test]
    fn lead_and_trail_shrink_window() {
        let t = tt(vec![brk("12:00", "13:00", "午休")]);
        let w = derive_windows(
            &t,
            WindowSpec {
                min_minutes: 10,
                lead_minutes: 5,
                trail_minutes: 10,
                max_concurrent: 2,
            },
        );
        assert_eq!(w[0].start, "12:05");
        assert_eq!(w[0].end, "12:50");
        assert_eq!(w[0].max_concurrent, 2);
    }

    #[test]
    fn window_at_matches() {
        let t = tt(vec![brk("12:10", "13:30", "午休")]);
        let w = derive_windows(&t, WindowSpec::default());
        assert!(window_at(&w, 12 * 60 + 30).is_some());
        assert!(window_at(&w, 12 * 60).is_none());
        assert!(window_at(&w, 13 * 60 + 30).is_none());
    }

    #[test]
    fn validate_detects_overlap() {
        let a = ProcessingWindow {
            name: "A".into(),
            start: "12:00".into(),
            end: "13:00".into(),
            scope: "afternoon".into(),
            max_concurrent: 1,
        };
        let b = ProcessingWindow {
            name: "B".into(),
            start: "12:30".into(),
            end: "13:30".into(),
            scope: "afternoon".into(),
            max_concurrent: 1,
        };
        let issues = validate_windows(&[a, b]);
        assert!(issues.iter().any(|s| s.contains("重叠")));
    }

    #[test]
    fn viable_break_threshold() {
        assert!(is_viable_break(&brk("12:00", "13:00", "x"), 15));
        assert!(!is_viable_break(&brk("12:00", "12:05", "x"), 15));
        assert!(!is_viable_break(&class(1, "12:00", "13:00"), 15));
    }

    #[test]
    fn scope_for_lesson_by_time() {
        assert_eq!(scope_for_lesson(8 * 60), "morning");
        assert_eq!(scope_for_lesson(14 * 60), "afternoon");
        assert_eq!(scope_for_lesson(20 * 60), "evening");
    }
}
