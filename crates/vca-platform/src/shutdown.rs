//! 优雅退出信号。
//!
//! **为什么必须有它**：默认情况下 Ctrl+C 会立即终止进程，Rust 的析构函数
//! **不会执行**于是 `CaptureSession::drop` 里「停掉 ffmpeg」的逻辑就跑了空，
//! 结果是一个孤儿 ffmpeg 进程 + 一个可能损坏的录像文件。
//!
//! 做法：注册 `SetConsoleCtrlHandler`，收到 Ctrl+C / 关窗时**只置一个标志**，
//! 让主循环自己发现并走正常退出路径（跑完所有 `Drop`）。

use std::sync::atomic::{AtomicBool, Ordering};

/// 是否已收到退出请求。
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// 查询是否收到退出请求。
pub fn is_shutdown_requested() -> bool {
    SHUTDOWN.load(Ordering::SeqCst)
}

/// 主动请求退出（供测试或内部使用）。
pub fn request_shutdown() {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

/// 复位标志（仅供测试）。
pub fn reset() {
    SHUTDOWN.store(false, Ordering::SeqCst);
}

/// 安装控制台控制处理器。返回是否安装成功。
///
/// 处理的事件：
/// - `CTRL_C_EVENT`(0)：Ctrl+C
/// - `CTRL_BREAK_EVENT`(1)：Ctrl+Break
/// - `CTRL_CLOSE_EVENT`(2)：关闭控制台窗口
/// - `CTRL_LOGOFF_EVENT`(5) / `CTRL_SHUTDOWN_EVENT`(6)：注销 / 关机
///
/// 返回 1 表示「已处理」，系统就不会立即杀进程，主循环有时间优雅收尾。
#[cfg(windows)]
pub fn install() -> bool {
    extern "system" fn handler(_ctrl_type: u32) -> i32 {
        SHUTDOWN.store(true, Ordering::SeqCst);
        1
    }

    extern "system" {
        fn SetConsoleCtrlHandler(handler: Option<extern "system" fn(u32) -> i32>, add: i32) -> i32;
    }

    // SAFETY: 传入的是符合 Win32 约定的函数指针；handler 内部只做一个原子写，
    // 不分配内存、不加锁，满足「控制台控制处理器不得做复杂操作」的约束。
    unsafe { SetConsoleCtrlHandler(Some(handler), 1) != 0 }
}

/// 非 Windows 平台：无控制台控制处理器。
#[cfg(not(windows))]
pub fn install() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_toggles() {
        reset();
        assert!(!is_shutdown_requested());
        request_shutdown();
        assert!(is_shutdown_requested());
        reset();
        assert!(!is_shutdown_requested());
    }

    #[test]
    fn install_does_not_panic() {
        // 无控制台时可能返回 false，但绝不能 panic
        let _ = install();
    }
}
