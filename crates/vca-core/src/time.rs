//! 本地日期时间与纯日期运算（不依赖第三方库）。
//!
//! 为什么不引入 chrono：本机 windows-gnu 工具链下，部分日期库会经
//! `iana-time-zone` 引入 raw-dylib 链接需求，导致 dlltool 失败。
//! 本模块以标准库 + 一个 Win32 调用实现所需能力，零依赖、可单测。

use std::fmt;

/// 公历日期。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LocalDate {
    /// 年。
    pub year: i32,
    /// 月（1-12）。
    pub month: u32,
    /// 日（1-31）。
    pub day: u32,
}

impl LocalDate {
    /// 构造日期。
    pub fn new(year: i32, month: u32, day: u32) -> Self {
        Self { year, month, day }
    }

    /// 距 1970-01-01 的天数（Howard Hinnant 的 civil 算法）。
    pub fn to_days(self) -> i64 {
        let y = self.year as i64 - if self.month <= 2 { 1 } else { 0 };
        let m = self.month as i64;
        let d = self.day as i64;
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let mp = (m + 9) % 12;
        let doy = (153 * mp + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe - 719468
    }

    /// 由天数还原日期。
    pub fn from_days(z: i64) -> Self {
        let z = z + 719468;
        let era = if z >= 0 { z } else { z - 146096 } / 146097;
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = y + if m <= 2 { 1 } else { 0 };
        Self {
            year: year as i32,
            month: m as u32,
            day: d as u32,
        }
    }

    /// 星期几：0 = 周一，6 = 周日。
    pub fn weekday(self) -> u32 {
        (self.to_days() + 3).rem_euclid(7) as u32
    }

    /// 该日期所在的 ISO 周键，形如 `2025-11`，用于存档命名。
    pub fn week_key(self) -> String {
        // 以该周的周四所在年份作为 ISO 年，保证跨年周稳定
        let wd = self.weekday() as i64; // 0=Mon
        let thursday = LocalDate::from_days(self.to_days() - wd + 3);
        let jan1 = LocalDate::new(thursday.year, 1, 1);
        let week = (thursday.to_days() - jan1.to_days()) / 7 + 1;
        format!("{:04}-{:02}", thursday.year, week)
    }

    /// 加上若干天。
    pub fn add_days(self, days: i64) -> Self {
        LocalDate::from_days(self.to_days() + days)
    }

    /// 该月天数。
    pub fn days_in_month(self) -> u32 {
        match self.month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                if (self.year % 4 == 0 && self.year % 100 != 0) || self.year % 400 == 0 {
                    29
                } else {
                    28
                }
            }
            _ => 30,
        }
    }
}

impl fmt::Display for LocalDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// 本地日期时间（分钟精度对调度足够）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LocalDateTime {
    /// 日期。
    pub date: LocalDate,
    /// 小时（0-23）。
    pub hour: u32,
    /// 分钟（0-59）。
    pub minute: u32,
}

impl LocalDateTime {
    /// 构造。
    pub fn new(date: LocalDate, hour: u32, minute: u32) -> Self {
        Self { date, hour, minute }
    }

    /// 从「某日的第几分钟」构造（分钟可越界，会自动换算到相邻日期）。
    pub fn from_minutes(date: LocalDate, minutes: i64) -> Self {
        let day_shift = minutes.div_euclid(1440);
        let m = minutes.rem_euclid(1440);
        Self {
            date: date.add_days(day_shift),
            hour: (m / 60) as u32,
            minute: (m % 60) as u32,
        }
    }

    /// 当日第几分钟。
    pub fn minute_of_day(self) -> u32 {
        self.hour * 60 + self.minute
    }

    /// 距 1970-01-01 的总分钟数（用于跨日比较）。
    pub fn to_minutes(self) -> i64 {
        self.date.to_days() * 1440 + self.minute_of_day() as i64
    }

    /// 由总分钟数还原。
    pub fn from_total_minutes(total: i64) -> Self {
        let day = total.div_euclid(1440);
        let m = total.rem_euclid(1440);
        Self {
            date: LocalDate::from_days(day),
            hour: (m / 60) as u32,
            minute: (m % 60) as u32,
        }
    }

    /// 加上若干分钟。
    pub fn add_minutes(self, delta: i64) -> Self {
        Self::from_total_minutes(self.to_minutes() + delta)
    }

    /// 与另一时刻相差的分钟数（self - other）。
    pub fn diff_minutes(self, other: Self) -> i64 {
        self.to_minutes() - other.to_minutes()
    }

    /// 解析 `HH:mm` 为「当日第几分钟」。
    pub fn parse_hhmm(s: &str) -> Option<u32> {
        let (h, m) = s.trim().split_once(':')?;
        let h: u32 = h.trim().parse().ok()?;
        let m: u32 = m.trim().parse().ok()?;
        if h > 23 || m > 59 {
            return None;
        }
        Some(h * 60 + m)
    }

    /// 解析 `YYYY-MM-DDTHH:mm` 或 `YYYY-MM-DD HH:mm`。
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().replace('T', " ");
        let (d, t) = s.split_once(' ')?;
        let mut it = d.split('-');
        let y: i32 = it.next()?.trim().parse().ok()?;
        let mo: u32 = it.next()?.trim().parse().ok()?;
        let da: u32 = it.next()?.trim().parse().ok()?;
        let mins = Self::parse_hhmm(t)?;
        Some(Self::new(LocalDate::new(y, mo, da), mins / 60, mins % 60))
    }
}

impl fmt::Display for LocalDateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}T{:02}:{:02}", self.date, self.hour, self.minute)
    }
}

// ---------------------------------------------------------------------------
// serde：以字符串形式读写，保证配置文件里的日期是 `2025-03-18` 而不是结构体。
// ---------------------------------------------------------------------------

impl serde::Serialize for LocalDate {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for LocalDate {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        let t = s.trim();
        let mut it = t.split('-');
        let y = it
            .next()
            .and_then(|v| v.trim().parse::<i32>().ok())
            .ok_or_else(|| serde::de::Error::custom(format!("非法日期: {s}")))?;
        let m = it
            .next()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .ok_or_else(|| serde::de::Error::custom(format!("非法日期: {s}")))?;
        let d = it
            .next()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .ok_or_else(|| serde::de::Error::custom(format!("非法日期: {s}")))?;
        if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
            return Err(serde::de::Error::custom(format!("日期越界: {s}")));
        }
        Ok(LocalDate::new(y, m, d))
    }
}

impl serde::Serialize for LocalDateTime {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for LocalDateTime {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        LocalDateTime::parse(&s)
            .ok_or_else(|| serde::de::Error::custom(format!("非法日期时间: {s}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_roundtrip() {
        for &(y, m, d) in &[(1970, 1, 1), (2000, 2, 29), (2025, 3, 18), (1999, 12, 31)] {
            let date = LocalDate::new(y, m, d);
            assert_eq!(LocalDate::from_days(date.to_days()), date);
        }
    }

    #[test]
    fn weekday_is_correct() {
        // 1970-01-01 是周四
        assert_eq!(LocalDate::new(1970, 1, 1).weekday(), 3);
        // 2025-03-17 是周一
        assert_eq!(LocalDate::new(2025, 3, 17).weekday(), 0);
        // 2025-03-23 是周日
        assert_eq!(LocalDate::new(2025, 3, 23).weekday(), 6);
    }

    #[test]
    fn leap_year_february() {
        assert_eq!(LocalDate::new(2024, 2, 1).days_in_month(), 29);
        assert_eq!(LocalDate::new(2025, 2, 1).days_in_month(), 28);
    }

    #[test]
    fn add_minutes_crosses_day() {
        let t = LocalDateTime::new(LocalDate::new(2025, 3, 18), 23, 50);
        let n = t.add_minutes(20);
        assert_eq!(n.date, LocalDate::new(2025, 3, 19));
        assert_eq!(n.hour, 0);
        assert_eq!(n.minute, 10);
    }

    #[test]
    fn parse_hhmm_validates() {
        assert_eq!(LocalDateTime::parse_hhmm("08:45"), Some(525));
        assert_eq!(LocalDateTime::parse_hhmm(" 7:05 "), Some(425));
        assert_eq!(LocalDateTime::parse_hhmm("24:00"), None);
        assert_eq!(LocalDateTime::parse_hhmm("08:75"), None);
        assert_eq!(LocalDateTime::parse_hhmm("bad"), None);
    }

    #[test]
    fn parse_datetime_both_separators() {
        let a = LocalDateTime::parse("2025-03-18T08:45").unwrap();
        let b = LocalDateTime::parse("2025-03-18 08:45").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.minute_of_day(), 525);
    }

    #[test]
    fn diff_minutes_is_signed() {
        let a = LocalDateTime::new(LocalDate::new(2025, 3, 18), 10, 0);
        let b = LocalDateTime::new(LocalDate::new(2025, 3, 18), 9, 30);
        assert_eq!(a.diff_minutes(b), 30);
        assert_eq!(b.diff_minutes(a), -30);
    }
}
