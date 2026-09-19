//! 本机时钟（平台层，承载唯一的一处 FFI）。
//!
//! 放在平台层而非核心层，是为了让 `vca-core` 保持 `#![forbid(unsafe_code)]`。

use vca_core::time::{LocalDate, LocalDateTime};

/// 读取本机当前本地时间（分钟精度）。
#[cfg(windows)]
pub fn now_local() -> LocalDateTime {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    extern "system" {
        fn GetLocalTime(lp_system_time: *mut SystemTime);
    }
    let mut st = SystemTime::default();
    // SAFETY: GetLocalTime 只写入调用方提供的、大小与 SYSTEMTIME 一致的缓冲区。
    unsafe { GetLocalTime(&mut st) };
    LocalDateTime::new(
        LocalDate::new(st.year as i32, st.month as u32, st.day as u32),
        st.hour as u32,
        st.minute as u32,
    )
}

/// 非 Windows 退化实现（UTC）；仅用于开发与测试。
#[cfg(not(windows))]
pub fn now_local() -> LocalDateTime {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    LocalDateTime::from_total_minutes(secs / 60)
}

/// 读秒级本地时间（返回 时, 分, 秒），用于需要更高精度的场合。
#[cfg(windows)]
pub fn now_local_secs() -> (LocalDateTime, u32) {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    extern "system" {
        fn GetLocalTime(lp_system_time: *mut SystemTime);
    }
    let mut st = SystemTime::default();
    // SAFETY: 同上，缓冲区大小与 SYSTEMTIME 一致。
    unsafe { GetLocalTime(&mut st) };
    (
        LocalDateTime::new(
            LocalDate::new(st.year as i32, st.month as u32, st.day as u32),
            st.hour as u32,
            st.minute as u32,
        ),
        st.second as u32,
    )
}

/// 非 Windows 退化实现。
#[cfg(not(windows))]
pub fn now_local_secs() -> (LocalDateTime, u32) {
    (now_local(), 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_is_plausible() {
        let t = now_local();
        assert!(t.date.year >= 2000, "年份异常: {}", t.date.year);
        assert!(t.hour < 24);
        assert!(t.minute < 60);
    }
}
