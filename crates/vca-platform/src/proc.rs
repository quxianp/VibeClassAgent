//! 子进程辅助：把「静默启动」的细节收敛到一处。
//!
//! 本项目对外部程序的静默性要求很硬（老师面前不能弹任何窗口），
//! 而 `CREATE_NO_WINDOW`、stdio 归零这些标志写错一个就会出现黑框一闪，
//! 所以统一从 [`silent`] 构造命令，不要在业务代码里各写各的。

use std::ffi::OsStr;
use std::process::{Command, Stdio};

/// Windows 的 `CREATE_NO_WINDOW`：不为控制台程序创建控制台窗口。
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 构造一个**完全静默**的命令：不弹窗、不读键盘、不写控制台。
///
/// 需要捕获输出（比如要解析 stdout 或保存日志）时，
/// 拿返回值自行改 `stdout`/`stderr` 即可，`CREATE_NO_WINDOW` 已经设好。
pub fn silent(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// 外部工具目录的候选位置（按优先级）。
///
/// 之所以要有多个候选，是因为程序在两种形态下的相对位置不同：
///
/// - **发布形态**：`vca.exe` 与 `tools/` 同级（打成 dist 包之后就是这样）；
/// - **开发形态**：`vca.exe` 在 `target/debug/`，而 `tools/` 在仓库根目录。
///
/// 只按「exe 所在目录」查找会导致开发时永远找不到 ffmpeg 和 whisper，
/// 而只按「当前工作目录」查找又会在把程序装到别处后失效。
pub fn tool_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(d) = std::env::var("VCA_TOOLS_DIR") {
        if !d.trim().is_empty() {
            dirs.push(std::path::PathBuf::from(d));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(p) = exe.parent() {
            dirs.push(p.join("tools"));
            // exe 在 target/debug 或 target/release 时，仓库根在往上两级
            if let Some(gp) = p.parent().and_then(|x| x.parent()) {
                dirs.push(gp.join("tools"));
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join("tools"));
    }
    dirs
}

/// 在候选工具目录里找相对路径 `rel`（如 `whisper/whisper-cli.exe`）。
pub fn find_tool(rel: &str) -> Option<std::path::PathBuf> {
    tool_dirs()
        .into_iter()
        .map(|d| d.join(rel))
        .find(|p| p.is_file())
}

/// 在 PATH 里找一个可执行文件。
pub fn which(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for ext in [".exe", ".cmd", ".bat", ""] {
            let cand = dir.join(format!("{name}{ext}"));
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silent_targets_the_given_program() {
        // 不断言 Debug 格式里的 stdio 字段 —— 那是标准库的内部表示，
        // 换版本就可能变。真正要验证的是「命令指向了传入的程序」，
        // 以及下面的 silent_runs_without_window 里真跑一次。
        let cmd = silent("cmd");
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("cmd"), "命令应指向传入的程序: {dbg}");
    }

    #[cfg(windows)]
    #[test]
    fn silent_runs_without_window() {
        // 真跑一次：静默执行 `cmd /c exit 0` 应当成功返回。
        let status = silent("cmd").args(["/c", "exit", "0"]).status();
        assert!(status.map(|s| s.success()).unwrap_or(false));
    }
}
