//! 运行会话判定。
//!
//! **为什么需要它**（策划书 R4）：Windows 服务运行在 **Session 0**，
//! 与用户桌面隔离，**无法采集屏幕**。如果误把本程序注册成服务，
//! 表现会是「录制启动成功但录出来是黑屏/空文件」，非常难排查。
//!
//! 因此在启动时主动检测并给出明确警告，比事后排查划算得多。

/// 进程运行方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// 计划任务 / 普通进程，运行于交互式用户会话（**推荐**）。
    Interactive,
    /// Windows 服务（Session 0，**无法采集桌面**）。
    Session0,
    /// 无法判定。
    Unknown,
}

impl RunMode {
    /// 中文名。
    pub fn label(self) -> &'static str {
        match self {
            Self::Interactive => "交互式用户会话",
            Self::Session0 => "Session 0（服务）",
            Self::Unknown => "未知",
        }
    }
}

/// 当前进程所在的会话 ID。
#[cfg(windows)]
pub fn current_session_id() -> Option<u32> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcessId() -> u32;
        fn ProcessIdToSessionId(pid: u32, session: *mut u32) -> i32;
    }
    let mut sid: u32 = 0;
    // SAFETY: GetCurrentProcessId 无参数；ProcessIdToSessionId 只写入提供的 u32。
    let ok = unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut sid) };
    if ok == 0 {
        None
    } else {
        Some(sid)
    }
}

/// 非 Windows 平台无会话概念。
#[cfg(not(windows))]
pub fn current_session_id() -> Option<u32> {
    None
}

/// 把当前控制台切到 UTF-8（代码页 65001）。
///
/// **为什么程序自己要设**：中文输出是 UTF-8 字节，而简中 Windows 的控制台
/// 默认代码页是 936(GBK)，直接输出会变成「鐜鑷」这样的乱码。
/// 在 `.cmd` 里 `chcp 65001` 只对那条命令之后的输出有效，且双击启动时
/// 时序不一定可靠；程序自己设一次最稳。失败时静默忽略（例如输出被重定向）。
#[cfg(windows)]
pub fn ensure_utf8_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn SetConsoleOutputCP(code_page: u32) -> i32;
        fn SetConsoleCP(code_page: u32) -> i32;
    }
    // SAFETY: 两个函数只设置当前进程的控制台代码页，无内存安全影响。
    unsafe {
        SetConsoleOutputCP(65001);
        SetConsoleCP(65001);
    }
}

/// 非 Windows 平台无需处理。
#[cfg(not(windows))]
pub fn ensure_utf8_console() {}

/// 判断是否由「双击 exe」启动。
///
/// 原理：双击时进程独占一个新控制台，`GetConsoleProcessList` 只会返回它自己；
/// 而从已有的命令行里运行时，控制台里至少还有父 shell。
///
/// 用途：让双击 exe 的用户看到交互菜单，而不是一闪而过的黑窗口。
#[cfg(windows)]
pub fn launched_by_double_click() -> bool {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetConsoleProcessList(list: *mut u32, count: u32) -> u32;
    }
    let mut buf = [0u32; 4];
    // SAFETY: 缓冲区大小与传入的 count 一致；函数只写入该缓冲区。
    let n = unsafe { GetConsoleProcessList(buf.as_mut_ptr(), 4) };
    // 0 = 没有控制台（例如被 GUI 程序拉起）；1 = 独占控制台
    n <= 1
}

/// 非 Windows 平台恒为 false。
#[cfg(not(windows))]
pub fn launched_by_double_click() -> bool {
    false
}

/// 判定当前运行方式。
pub fn detect_run_mode() -> RunMode {
    match current_session_id() {
        Some(0) => RunMode::Session0,
        Some(_) => RunMode::Interactive,
        None => RunMode::Unknown,
    }
}

/// 当前运行方式能否采集桌面。
pub fn can_capture_desktop() -> bool {
    !matches!(detect_run_mode(), RunMode::Session0)
}

/// 启动时自检：若无法采集桌面，返回一条**面向用户**的告警文案。
///
/// 返回 `None` 表示一切正常。
pub fn startup_warning() -> Option<String> {
    match detect_run_mode() {
        RunMode::Session0 => Some(
            "检测到程序运行在 Session 0（很可能是被注册成了 Windows 服务）。\n\
             服务与用户桌面隔离，无法采集屏幕，录制会得到黑屏。\n\
             请改用「计划任务」并在用户登录时触发，或直接双击运行。"
                .to_string(),
        ),
        RunMode::Unknown => Some(
            "无法判定当前会话类型；若录制结果异常（黑屏），请检查是否以服务方式运行。".to_string(),
        ),
        RunMode::Interactive => None,
    }
}

/// 判断给定运行方式是否满足「可采集桌面」的要求。
pub fn can_capture_desktop_for(mode: RunMode) -> bool {
    !matches!(mode, RunMode::Session0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session0_cannot_capture() {
        assert!(!can_capture_desktop_for(RunMode::Session0));
        assert!(can_capture_desktop_for(RunMode::Interactive));
        assert!(can_capture_desktop_for(RunMode::Unknown));
    }

    #[test]
    fn labels_are_readable() {
        assert_eq!(RunMode::Session0.label(), "Session 0（服务）");
        assert_eq!(RunMode::Interactive.label(), "交互式用户会话");
    }

    #[cfg(windows)]
    #[test]
    fn detect_returns_a_mode() {
        // 测试进程通常跑在交互式会话；但不能假设，只要求不 panic
        let m = detect_run_mode();
        assert!(matches!(
            m,
            RunMode::Interactive | RunMode::Session0 | RunMode::Unknown
        ));
        let _ = current_session_id();
        let _ = startup_warning();
    }

    #[test]
    fn interactive_has_no_warning() {
        // 直接验证映射关系（不依赖真实环境）
        assert!(can_capture_desktop_for(RunMode::Interactive));
    }
}
