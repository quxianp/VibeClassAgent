//! DeepSeek 错峰计费时段判定。
//!
//! 官方规则（见 <https://api-docs.deepseek.com/quick_start/pricing/>）：
//!
//! > **高峰时段是 UTC 01:00–04:00 与 06:00–10:00，周一至周五，排除中国法定节假日；
//! > 其余所有时间都按闲时计费（半价），包括周末与中国法定节假日全天。**
//!
//! 换算成北京时间（UTC+8）就是**工作日 09:00–12:00 与 14:00–18:00** ——
//! 正好是上课时间。也就是说「上课录、午休与晚餐时段处理」这个默认安排
//! 天然落在闲时，只有把处理窗口排进上课时段、或用户手动触发时才会撞上高峰。
//!
//! 这里只做判定，不做节流：daemon 命中处理窗口时若发现正处于高峰，
//! 就把这批作业积压下来、等进入闲时再补跑（见 `vca-engine` 的 daemon）。
//! 该行为**默认开启**，用户可用 `llm.defer_on_peak: false` 关掉。
//!
//! 节假日按**本地日期**判断，由调用方传入：中国的法定节假日是按本地日历定的，
//! 而高峰时段是按 UTC 小时定的，两者不能混在一起算。

use std::time::{SystemTime, UNIX_EPOCH};

/// 高峰时段（UTC 小时，半开区间 `[start, end)`）。
const PEAK_UTC_HOURS: &[(u32, u32)] = &[(1, 4), (6, 10)];

/// Unix 时间戳 → UTC 星期（0 = 周日，与 C 的 `tm_wday` 一致）。
fn utc_weekday(unix_secs: u64) -> u32 {
    // 1970-01-01 是星期四，所以 +4 后 0 恰好落在周日
    ((unix_secs / 86_400 + 4) % 7) as u32
}

/// 某个 UTC 时刻是否处于 DeepSeek 高峰。
pub fn is_peak_utc(unix_secs: u64, holiday: bool) -> bool {
    if holiday {
        return false; // 法定节假日全天按闲时计费
    }
    let wd = utc_weekday(unix_secs);
    if wd == 0 || wd == 6 {
        return false; // 周末全天闲时
    }
    let hour = ((unix_secs % 86_400) / 3_600) as u32;
    PEAK_UTC_HOURS.iter().any(|(s, e)| hour >= *s && hour < *e)
}

/// 现在是否处于高峰。
pub fn is_peak_now(holiday: bool) -> bool {
    is_peak_utc(now_unix(), holiday)
}

/// 距离下一个闲时还有多少秒（0 = 现在就是闲时）。
pub fn secs_until_offpeak(unix_secs: u64, holiday: bool) -> u64 {
    if !is_peak_utc(unix_secs, holiday) {
        return 0;
    }
    let day_start = unix_secs - unix_secs % 86_400;
    let hour = ((unix_secs % 86_400) / 3_600) as u32;
    for (s, e) in PEAK_UTC_HOURS {
        if hour >= *s && hour < *e {
            let end = day_start + u64::from(*e) * 3_600;
            return end.saturating_sub(unix_secs);
        }
    }
    0
}

/// 同上，取当前时刻。
pub fn secs_until_offpeak_now(holiday: bool) -> u64 {
    secs_until_offpeak(now_unix(), holiday)
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 端点像不像 DeepSeek —— 只有它有错峰计费，别的厂商没必要跟着延后。
///
/// 判断依据是厂商 id 或 base_url 里出现 `deepseek`：
/// 用户把 base_url 指向自建代理时，只要域名里还带着它就能认出来。
pub fn looks_like_deepseek(base_url: &str, provider: &str) -> bool {
    if provider.trim().eq_ignore_ascii_case("deepseek") {
        return true;
    }
    base_url.to_ascii_lowercase().contains("deepseek")
}

/// 综合判断此刻要不要**因为高峰而暂缓**调用模型。
///
/// 抽成纯函数是为了能测：这三个条件的组合（开关 / 是不是 DeepSeek / 是不是高峰）
/// 一旦写反，表现是「要么白等半天、要么白花一倍钱」，两种都不会在日志里明显暴露。
pub fn should_defer(
    defer_on_peak: bool,
    base_url: &str,
    provider: &str,
    holiday: bool,
    unix_secs: u64,
) -> bool {
    defer_on_peak && looks_like_deepseek(base_url, provider) && is_peak_utc(unix_secs, holiday)
}

/// 同上，取当前时刻。
pub fn should_defer_now(
    defer_on_peak: bool,
    base_url: &str,
    provider: &str,
    holiday: bool,
) -> bool {
    should_defer(defer_on_peak, base_url, provider, holiday, now_unix())
}

/// 把秒数说成人话，用于日志里提示「还有多久进入闲时」。
pub fn humanize_secs(secs: u64) -> String {
    let m = secs / 60;
    if m == 0 {
        return "不到 1 分钟".to_string();
    }
    if m < 60 {
        return format!("约 {m} 分钟");
    }
    let h = m / 60;
    let mm = m % 60;
    if mm == 0 {
        format!("约 {h} 小时")
    } else {
        format!("约 {h} 小时 {mm} 分钟")
    }
}

/// 高峰积压队列：记录哪些 scope 因为撞上高峰而被推迟。
///
/// 抽成小结构体是为了能测「补跑」语义。这段逻辑原本写在 daemon 主循环里，
/// 出错的表现（该补的没补、或者反复补跑重活）都不容易从日志里看出来。
#[derive(Debug, Default)]
pub struct DeferQueue {
    scopes: Vec<String>,
}

impl DeferQueue {
    /// 积压一个 scope。
    ///
    /// 返回 `true` 表示这是**首次**积压 —— 调用方据此只打一次日志，
    /// 否则主循环每 30 秒就会重复刷一条同样的提示。
    pub fn defer(&mut self, scope: &str) -> bool {
        if self.scopes.iter().any(|s| s == scope) {
            return false;
        }
        self.scopes.push(scope.to_string());
        true
    }

    /// 高峰结束后取走全部待补跑的 scope（队列随之清空）。
    ///
    /// 仍在高峰、或队列本来就空时返回空列表 —— 前者的意思是「时候未到」。
    pub fn take_ready(&mut self, peak: bool) -> Vec<String> {
        if peak || self.scopes.is_empty() {
            return Vec::new();
        }
        std::mem::take(&mut self.scopes)
    }

    /// 队列是否为空。
    pub fn is_empty(&self) -> bool {
        self.scopes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// UTC 年月日时分 → Unix 时间戳（Howard Hinnant 的 days_from_civil）。
    ///
    /// 自己算而不是引 chrono：这里只需要几个固定时刻，为测试拉一个日期库不值。
    fn ts(y: i64, m: i64, d: i64, h: u64, mi: u64) -> u64 {
        let y = if m <= 2 { y - 1 } else { y };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = (y - era * 400) as u64;
        let mp = ((m + 9) % 12) as u64;
        let doy = (153 * mp + 2) / 5 + (d as u64) - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = (era * 146_097) as u64 + doe - 719_468;
        days * 86_400 + h * 3_600 + mi * 60
    }

    // 2026-09-21 是周一，2026-09-20 是周日（daemon 实测日志可佐证）

    #[test]
    fn weekday_peak_windows_are_peak() {
        assert!(is_peak_utc(ts(2026, 9, 21, 2, 0), false));
        assert!(is_peak_utc(ts(2026, 9, 21, 1, 0), false));
        assert!(is_peak_utc(ts(2026, 9, 21, 3, 59), false));
        assert!(is_peak_utc(ts(2026, 9, 21, 7, 30), false));
        assert!(is_peak_utc(ts(2026, 9, 21, 9, 59), false));
    }

    #[test]
    fn boundaries_are_half_open() {
        // 04:00 与 10:00 已经是闲时，00:59 还没进高峰
        assert!(!is_peak_utc(ts(2026, 9, 21, 4, 0), false));
        assert!(!is_peak_utc(ts(2026, 9, 21, 6, 0) - 60, false));
        assert!(!is_peak_utc(ts(2026, 9, 21, 10, 0), false));
        assert!(!is_peak_utc(ts(2026, 9, 21, 5, 0), false));
        assert!(!is_peak_utc(ts(2026, 9, 21, 11, 0), false));
    }

    #[test]
    fn weekend_is_offpeak_all_day() {
        for h in [2, 7, 9] {
            assert!(
                !is_peak_utc(ts(2026, 9, 20, h, 0), false),
                "周日 {h} 点不该是高峰"
            );
            assert!(
                !is_peak_utc(ts(2026, 9, 26, h, 0), false),
                "周六 {h} 点不该是高峰"
            );
        }
    }

    #[test]
    fn holiday_overrides_weekday() {
        // 同是周一 02:00，标成节假日就必须按闲时算
        assert!(is_peak_utc(ts(2026, 9, 21, 2, 0), false));
        assert!(!is_peak_utc(ts(2026, 9, 21, 2, 0), true));
    }

    #[test]
    fn countdown_points_at_window_end() {
        assert_eq!(secs_until_offpeak(ts(2026, 9, 21, 2, 0), false), 2 * 3_600);
        assert_eq!(secs_until_offpeak(ts(2026, 9, 21, 7, 0), false), 3 * 3_600);
        assert_eq!(secs_until_offpeak(ts(2026, 9, 21, 5, 0), false), 0);
        assert_eq!(secs_until_offpeak(ts(2026, 9, 20, 2, 0), false), 0);
    }

    #[test]
    fn detects_deepseek_endpoints() {
        assert!(looks_like_deepseek("https://api.deepseek.com/v1", ""));
        assert!(looks_like_deepseek("", "deepseek"));
        assert!(looks_like_deepseek("", "DeepSeek"));
        assert!(looks_like_deepseek(
            "https://my-proxy.example/deepseek/v1",
            "custom"
        ));
        assert!(!looks_like_deepseek("https://api.moonshot.cn/v1", "kimi"));
        assert!(!looks_like_deepseek("", ""));
    }

    #[test]
    fn defer_only_when_all_three_hold() {
        let peak = ts(2026, 9, 21, 2, 0);
        let off = ts(2026, 9, 21, 5, 0);
        let ds = "https://api.deepseek.com/v1";
        assert!(should_defer(true, ds, "", false, peak));
        assert!(
            !should_defer(false, ds, "", false, peak),
            "开关关了就不该延后"
        );
        assert!(
            !should_defer(true, "https://api.moonshot.cn/v1", "kimi", false, peak),
            "不是 DeepSeek 不延后"
        );
        assert!(
            !should_defer(true, ds, "", false, off),
            "闲时不该延后 —— 那只是白白晚交作业"
        );
        assert!(!should_defer(true, ds, "", true, peak), "法定节假日不延后");
    }

    #[test]
    fn defer_queue_dedupes_and_flushes_on_offpeak() {
        let mut q = DeferQueue::default();
        assert!(q.defer("morning"), "首次积压返回 true（调用方据此打日志）");
        assert!(!q.defer("morning"), "同一个 scope 不重复记，否则日志会刷屏");
        assert!(!q.is_empty());
        assert!(q.take_ready(true).is_empty(), "还在高峰就不该取走");
        assert!(!q.is_empty(), "没取走说明它还留着，下轮还能补跑");
        assert_eq!(q.take_ready(false), vec!["morning".to_string()]);
        assert!(q.is_empty(), "取走后清空，避免重复补跑同一批重活");
        assert!(q.take_ready(false).is_empty(), "空队列再取还是空");
    }

    #[test]
    fn humanize_reads_naturally() {
        assert_eq!(humanize_secs(30), "不到 1 分钟");
        assert_eq!(humanize_secs(20 * 60), "约 20 分钟");
        assert_eq!(humanize_secs(3_600), "约 1 小时");
        assert_eq!(humanize_secs(3_600 + 1_200), "约 1 小时 20 分钟");
    }
}
