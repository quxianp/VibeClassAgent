//! 硬件与系统负载探测（用于**自适应降档**）。
//!
//! 设计依据（策划书 3.2.1 / 7.2）：
//! 采集前与录制中定期复评系统负载，负载高则降 fps / 升 CRF / 降分辨率。
//!
//! 实现方式（全部是系统自带 API，零第三方依赖）：
//! - CPU：`GetSystemTimes` 两次采样求增量（内核时间含空闲时间，需减去）
//! - 内存：`GlobalMemoryStatusEx`
//! - 磁盘：`GetDiskFreeSpaceExW`
//!
//! CPU 占用需要**两次采样**才有意义，因此本模块在内部保存上一次读数；
//! 首次调用会立即再采一次（约 200 ms 间隔）以得到一个有效值。

use std::sync::Mutex;

/// 系统负载快照。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoadSnapshot {
    /// CPU 占用百分比（0-100）。
    pub cpu_percent: f32,
    /// 可用内存（MB）。
    pub mem_available_mb: u64,
    /// 磁盘剩余空间（MB）。
    pub disk_free_mb: u64,
    /// 内存占用百分比（0-100）。
    pub mem_percent: f32,
}

impl Default for LoadSnapshot {
    fn default() -> Self {
        Self {
            cpu_percent: 0.0,
            mem_available_mb: 0,
            disk_free_mb: 0,
            mem_percent: 0.0,
        }
    }
}

/// 负载等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadLevel {
    /// 空闲，可用最高画质。
    Low,
    /// 中等，适度降档。
    Medium,
    /// 高负载，降到最低档。
    High,
}

impl LoadLevel {
    /// 依据快照判断档位。
    ///
    /// 判据（**满足任一即升级**）：
    /// - CPU：`> 75%` 高；`> 50%` 中；
    /// - 内存占用：`> 90%` 高；`> 75%` 中（内存吃紧时再降画质会更糟，故阈值更高）。
    pub fn from_snapshot(s: &LoadSnapshot) -> Self {
        if s.cpu_percent > 75.0 || s.mem_percent > 90.0 {
            Self::High
        } else if s.cpu_percent > 50.0 || s.mem_percent > 75.0 {
            Self::Medium
        } else {
            Self::Low
        }
    }

    /// 中文名（日志用）。
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "低",
            Self::Medium => "中",
            Self::High => "高",
        }
    }
}

// ---------------------------------------------------------------------------
// Win32 绑定
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod win {
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    pub struct FileTime {
        pub low: u32,
        pub high: u32,
    }

    impl FileTime {
        pub fn as_u64(self) -> u64 {
            ((self.high as u64) << 32) | self.low as u64
        }
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    pub struct MemoryStatusEx {
        pub length: u32,
        pub memory_load: u32,
        pub total_phys: u64,
        pub avail_phys: u64,
        pub total_page_file: u64,
        pub avail_page_file: u64,
        pub total_virtual: u64,
        pub avail_virtual: u64,
        pub avail_extended_virtual: u64,
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetSystemTimes(
            idle: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        pub fn GlobalMemoryStatusEx(buf: *mut MemoryStatusEx) -> i32;
        pub fn GetDiskFreeSpaceExW(
            dir: *const u16,
            free_to_caller: *mut u64,
            total: *mut u64,
            total_free: *mut u64,
        ) -> i32;
    }
}

/// 上一次的 CPU 累计值（idle, kernel, user）。
#[cfg(windows)]
static LAST_CPU: Mutex<Option<(u64, u64, u64)>> = Mutex::new(None);

/// 读取一次原始 CPU 累计值。
#[cfg(windows)]
fn read_cpu_times() -> Option<(u64, u64, u64)> {
    use win::*;
    let mut idle = FileTime::default();
    let mut kernel = FileTime::default();
    let mut user = FileTime::default();
    // SAFETY: 三个指针都指向本函数内已初始化的栈变量。
    let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
    if ok == 0 {
        return None;
    }
    Some((idle.as_u64(), kernel.as_u64(), user.as_u64()))
}

/// 读取内存状态。
#[cfg(windows)]
fn read_memory() -> Option<(u64, f32)> {
    use win::*;
    let mut m = MemoryStatusEx {
        length: std::mem::size_of::<MemoryStatusEx>() as u32,
        ..Default::default()
    };
    // SAFETY: GlobalMemoryStatusEx 只写入调用方提供的缓冲区；
    // length 已按文档要求设置为结构体大小。
    let ok = unsafe { GlobalMemoryStatusEx(&mut m) };
    if ok == 0 {
        return None;
    }
    Some((m.avail_phys / (1024 * 1024), m.memory_load as f32))
}

/// 读取指定目录所在盘的剩余空间（MB）。
#[cfg(windows)]
fn read_disk_free(dir: &str) -> Option<u64> {
    use win::*;
    let w: Vec<u16> = dir.encode_utf16().chain(std::iter::once(0)).collect();
    let mut free_to_caller: u64 = 0;
    let mut total: u64 = 0;
    let mut total_free: u64 = 0;
    // SAFETY: 四个参数均为有效指针（字符串缓冲区以 NUL 结尾）。
    let ok = unsafe {
        GetDiskFreeSpaceExW(w.as_ptr(), &mut free_to_caller, &mut total, &mut total_free)
    };
    if ok == 0 {
        return None;
    }
    Some(total_free / (1024 * 1024))
}

/// 读取当前系统负载。
///
/// 首次调用为了拿到有效的 CPU 占用，会在内部间隔 200 ms 采两次样。
pub fn sample_load() -> anyhow::Result<LoadSnapshot> {
    sample_load_inner(std::path::Path::new("."), true)
}

/// 读取负载，并指定用于检测磁盘剩余空间的目录。
pub fn sample_load_for(dir: &std::path::Path) -> anyhow::Result<LoadSnapshot> {
    sample_load_inner(dir, true)
}

/// 内部实现；`allow_wait` 为真时首次采样会等待 200 ms。
pub fn sample_load_inner(dir: &std::path::Path, allow_wait: bool) -> anyhow::Result<LoadSnapshot> {
    #[cfg(windows)]
    {
        let mut snap = LoadSnapshot::default();

        // --- CPU ---
        let now = read_cpu_times();
        let prev = LAST_CPU.lock().ok().and_then(|g| *g);
        let cpu = match (prev, now) {
            (Some(p), Some(n)) => {
                if let Ok(mut g) = LAST_CPU.lock() {
                    *g = Some(n);
                }
                compute_cpu_usage(p, n).unwrap_or(0.0)
            }
            (None, Some(n)) => {
                if let Ok(mut g) = LAST_CPU.lock() {
                    *g = Some(n);
                }
                if allow_wait {
                    // 首次：短暂等待后重新采样，得到有效值
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    match read_cpu_times() {
                        Some(n2) => {
                            if let Ok(mut g) = LAST_CPU.lock() {
                                *g = Some(n2);
                            }
                            compute_cpu_usage(n, n2).unwrap_or(0.0)
                        }
                        None => 0.0,
                    }
                } else {
                    0.0
                }
            }
            _ => 0.0,
        };
        snap.cpu_percent = cpu;

        // --- 内存 ---
        if let Some((avail_mb, load)) = read_memory() {
            snap.mem_available_mb = avail_mb;
            snap.mem_percent = load;
        }

        // --- 磁盘 ---
        let dir_str = dir.to_string_lossy().to_string();
        let probe_dir = if dir_str.is_empty() {
            ".".to_string()
        } else {
            dir_str
        };
        if let Some(free) = read_disk_free(&probe_dir) {
            snap.disk_free_mb = free;
        }

        Ok(snap)
    }

    #[cfg(not(windows))]
    {
        let _ = (dir, allow_wait);
        Ok(LoadSnapshot::default())
    }
}

/// 由两次 CPU 累计值算占用率。
///
/// `GetSystemTimes` 的 kernel 时间**包含** idle，所以总时间是 `kernel + user`，
/// 而空闲时间是 `idle`。
#[cfg(windows)]
fn compute_cpu_usage(prev: (u64, u64, u64), now: (u64, u64, u64)) -> Option<f32> {
    let d_idle = now.0.checked_sub(prev.0)? as f64;
    let d_kernel = now.1.checked_sub(prev.1)? as f64;
    let d_user = now.2.checked_sub(prev.2)? as f64;
    let total = d_kernel + d_user;
    if total <= 0.0 {
        return Some(0.0);
    }
    let busy = (total - d_idle).max(0.0);
    Some(((busy / total) * 100.0).clamp(0.0, 100.0) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_thresholds() {
        let mk = |cpu: f32, mem: f32| LoadSnapshot {
            cpu_percent: cpu,
            mem_percent: mem,
            ..Default::default()
        };
        assert_eq!(LoadLevel::from_snapshot(&mk(10.0, 40.0)), LoadLevel::Low);
        assert_eq!(LoadLevel::from_snapshot(&mk(60.0, 40.0)), LoadLevel::Medium);
        assert_eq!(LoadLevel::from_snapshot(&mk(90.0, 40.0)), LoadLevel::High);
        // 内存吃紧也能升级
        assert_eq!(LoadLevel::from_snapshot(&mk(10.0, 80.0)), LoadLevel::Medium);
        assert_eq!(LoadLevel::from_snapshot(&mk(10.0, 95.0)), LoadLevel::High);
    }

    #[test]
    fn level_labels() {
        assert_eq!(LoadLevel::Low.label(), "低");
        assert_eq!(LoadLevel::Medium.label(), "中");
        assert_eq!(LoadLevel::High.label(), "高");
    }

    #[cfg(windows)]
    #[test]
    fn sample_returns_plausible_values() {
        let s = sample_load().unwrap();
        assert!(
            (0.0..=100.0).contains(&s.cpu_percent),
            "cpu={}",
            s.cpu_percent
        );
        assert!(s.mem_percent <= 100.0, "mem={}", s.mem_percent);
        // 本机一定有内存
        assert!(s.mem_available_mb > 0, "可用内存应大于 0");
        // 磁盘剩余也应能读到
        assert!(s.disk_free_mb > 0, "磁盘剩余应大于 0");
    }

    #[cfg(windows)]
    #[test]
    fn cpu_usage_is_bounded() {
        // 构造一个人工增量：总 100，空闲 30 -> 忙 70%
        let u = compute_cpu_usage((0, 0, 0), (30, 60, 40)).unwrap();
        assert!((u - 70.0).abs() < 0.01, "got {u}");
        // 完全空闲
        let u2 = compute_cpu_usage((0, 0, 0), (100, 100, 0)).unwrap();
        assert!(u2 < 0.01, "got {u2}");
        // 计数器不前进 -> 0，不应 panic
        assert_eq!(compute_cpu_usage((5, 5, 5), (5, 5, 5)), Some(0.0));
    }

    #[cfg(windows)]
    #[test]
    fn cpu_usage_handles_counter_reset() {
        // 计数器回退（理论上不该发生）必须返回 None 而不是 panic/溢出
        assert_eq!(compute_cpu_usage((100, 100, 100), (10, 10, 10)), None);
    }
}
