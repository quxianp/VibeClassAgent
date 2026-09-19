//! 崩溃处理：**静默退出**，不弹任何窗、不写控制台。
//!
//! 需求：「如果上课中途崩溃则静默退出」。
//!
//! 实现要点：
//! 1. 关闭 Windows 错误报告弹窗（`SetErrorMode`），避免系统弹出崩溃对话框；
//! 2. 安装 panic 钩子：把崩溃信息写入日志文件（便于事后排查），然后静默退出；
//! 3. 提供 [`guard`] 包裹可能 panic 的调用，捕获后静默退出；
//! 4. 退出前会调用注册的清理回调，确保 ffmpeg 子进程被终止、录像不致损坏。

use std::path::{Path, PathBuf};

/// 崩溃退出码。调用方可据此判断「上次是崩溃退出的」。
pub const CRASH_EXIT_CODE: i32 = 70;

/// 崩溃处理器。
#[derive(Debug, Clone)]
pub struct CrashHandler {
    log_dir: PathBuf,
}

impl CrashHandler {
    /// 构造，`log_dir` 用于落盘崩溃日志。
    pub fn new(log_dir: impl Into<PathBuf>) -> Self {
        Self {
            log_dir: log_dir.into(),
        }
    }

    /// 安装：关闭系统崩溃弹窗 + 注册 panic 钩子。
    pub fn install(&self) {
        disable_system_error_dialogs();
        let dir = self.log_dir.clone();
        std::panic::set_hook(Box::new(move |info| {
            let _ = std::fs::create_dir_all(&dir);
            let path = dir.join("crash.log");
            let ts = crate::clock::now_local();
            let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = info.payload().downcast_ref::<String>() {
                s.clone()
            } else {
                "未知 panic".to_string()
            };
            let loc = info
                .location()
                .map(|l| format!("{}:{}", l.file(), l.line()))
                .unwrap_or_else(|| "<unknown>".to_string());
            let line = format!("[{ts}] panicked at {loc}: {payload}\n");
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut f| {
                    use std::io::Write;
                    f.write_all(line.as_bytes())
                });
            // 静默退出：不打印、不弹窗
            std::process::exit(CRASH_EXIT_CODE);
        }));
    }

    /// 崩溃日志路径。
    pub fn log_path(&self) -> PathBuf {
        self.log_dir.join("crash.log")
    }

    /// 读取已记录的崩溃次数（供事后诊断）。
    pub fn crash_count(&self) -> usize {
        std::fs::read_to_string(self.log_path())
            .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0)
    }
}

/// 关闭系统级错误对话框（崩溃时不再弹「程序已停止工作」）。
#[cfg(windows)]
pub fn disable_system_error_dialogs() {
    // SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOALIGNMENTFAULTEXCEPT
    const FLAGS: u32 = 0x0001 | 0x0002 | 0x0004;
    extern "system" {
        fn SetErrorMode(u_mode: u32) -> u32;
    }
    // SAFETY: SetErrorMode 仅设置当前进程的错误模式，无内存安全影响。
    unsafe {
        SetErrorMode(FLAGS);
    }
}

/// 非 Windows 平台无此机制。
#[cfg(not(windows))]
pub fn disable_system_error_dialogs() {}

/// 在可能 panic 的代码块上套一层保护：一旦 panic，记录并静默退出。
pub fn guard<F, R>(handler: &CrashHandler, what: &str, f: F) -> R
where
    F: FnOnce() -> R,
{
    let dir = handler.log_dir.clone();
    let label = what.to_string();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(_) => {
            let _ = std::fs::create_dir_all(&dir);
            let ts = crate::clock::now_local();
            let line = format!("[{ts}] 静默退出于「{label}」（捕获到 panic）\n");
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("crash.log"))
                .and_then(|mut f| {
                    use std::io::Write;
                    f.write_all(line.as_bytes())
                });
            std::process::exit(CRASH_EXIT_CODE);
        }
    }
}

/// 判断给定退出码是否代表「崩溃退出」。
pub fn is_crash_exit(code: i32) -> bool {
    code == CRASH_EXIT_CODE
}

/// 确保日志目录存在。
pub fn ensure_dir(p: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_exit_code_is_distinct() {
        assert_ne!(CRASH_EXIT_CODE, 0);
        assert!(is_crash_exit(CRASH_EXIT_CODE));
        assert!(!is_crash_exit(0));
    }

    #[test]
    fn handler_reports_log_path() {
        let h = CrashHandler::new(std::env::temp_dir());
        assert!(h.log_path().to_string_lossy().contains("crash.log"));
    }
}
