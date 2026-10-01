//! 控制台窗口的隐藏。
//!
//! # 这个黑框是从哪来的
//!
//! `vca.exe` 是**控制台子系统**程序（Windows 里叫 `Console` subsystem）。
//! 这意味着双击它时系统会先给它分配一个控制台窗口 —— 哪怕这个程序
//! 接下来要干的事情是"打开一个图形界面"。
//!
//! 所以用户看到的现象是：双击 → 弹出一个黑框 → 几百毫秒后图形界面才出来，
//! 而那个黑框**一直留在那里**（因为进程还活着）。观感上就像个没做完的工具。
//!
//! # 为什么不能在 Cargo.toml 里直接改成 windows 子系统
//!
//! `#![windows_subsystem = "windows"]` 一开，**整个二进制**就与控制台脱钩：
//! `println!` 不再有任何输出，`vca doctor`、`vca plugin list` 这些
//! 本来靠 stdout 干活的子命令会直接哑掉 —— 而 CLI 是这个项目的重要用法
//! （HANDOFF 里明确写了脚本化部署与 `vca doctor` 自检）。
//!
//! 换句话说：**同一个可执行文件，GUI 模式要从控制台脱离，CLI 模式必须留在控制台**。
//! 这在编译期是互斥的，只能在运行时决定。
//!
//! # 运行时的做法
//!
//! 启动时判断"这次是 GUI 模式还是 CLI 模式"（有没有子命令），
//! GUI 模式就调 [`hide`]：
//!
//! 1. `GetConsoleWindow()` 取到属于**本进程**的控制台窗口句柄；
//! 2. 有的话 `ShowWindow(SW_HIDE)` 把它藏起来；
//! 3. 再 `FreeConsole()` 让进程与控制台彻底解绑。
//!
//! 只 `FreeConsole` 是不够的：控制台窗口并不会因此消失（它会留着，
//! 只是没有进程附着）。反过来只 `ShowWindow` 也不够：任务栏上可能还挂着一个
//! 条目，而且子进程继承控制台时还会冒出来。两个都做才干净。
//!
//! # 输出去哪了
//!
//! 隐藏控制台**不等于**丢掉日志。日志早就同时去了两个地方：
//! - `tracing` 的 layer 往内存环形缓冲写（[`crate::logbuf`]）→ 界面展示；
//! - fmt layer 往 `logs/ui.log` 写 → 事后排查。
//!
//! 控制台只是"第三个去处"，去掉它不影响前两个。这也是为什么
//! [`hide`] 能安全地被调用 —— 它不改变任何输出的内容，只改变呈现的位置。

/// 隐藏并释放本进程的控制台窗口。
///
/// 非 Windows 平台上是空操作。
///
/// 返回 `true` 表示**确实藏掉了一个控制台窗口**（调试时有用：
/// 你能确认"这次真的隐藏了"而不是"本来就没有"）。
#[cfg(windows)]
pub fn hide() -> bool {
    // 这些声明必须与 overlay.rs 里同名函数的签名**完全一致** ——
    // 包括句柄类型（那边用的是 isize，不是裸指针），否则链接期会报
    // "redeclared with a different signature"。
    type Hwnd = isize;
    type Bool = i32;

    #[link(name = "user32")]
    extern "system" {
        fn GetConsoleWindow() -> Hwnd;
        fn ShowWindow(hwnd: Hwnd, n_cmd_show: i32) -> Bool;
        fn IsWindowVisible(hwnd: Hwnd) -> Bool;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn FreeConsole() -> Bool;
    }
    /// `SW_HIDE`
    const SW_HIDE: i32 = 0;

    // SAFETY 说明：
    // - GetConsoleWindow 只读全局状态，无控制台时返回 0。
    // - 用句柄前一律判 0；句柄在 FreeConsole 之前一直有效。
    // - ShowWindow / IsWindowVisible 对无效句柄只是失败，不会崩。
    unsafe {
        let hwnd = GetConsoleWindow();
        if hwnd == 0 {
            // 已经是 windows 子系统、或输出被重定向时就是这个情况：
            // 本来就没有窗口可藏，不算错误。
            return false;
        }
        let was_visible = IsWindowVisible(hwnd) != 0;
        ShowWindow(hwnd, SW_HIDE);
        // 脱离控制台：这样后续子进程（ffmpeg 等）不会继承一个被藏起来的
        // 控制台，也就不会在别处冒出黑框来
        FreeConsole();
        was_visible
    }
}

/// 非 Windows：空操作。
#[cfg(not(windows))]
pub fn hide() -> bool {
    false
}

/// 本进程当前是否附着着控制台。
///
/// 用于自检与诊断：`vca doctor` 会报这一项。
/// GUI 模式下启动完成后应当是 `false`。
#[cfg(windows)]
pub fn has_console() -> bool {
    // 与 hide 保持同一套类型约定（isize 句柄，见那边的说明）
    type Hwnd = isize;
    #[link(name = "user32")]
    extern "system" {
        fn GetConsoleWindow() -> Hwnd;
    }
    // SAFETY: 只读全局状态，无参数传入。
    unsafe { GetConsoleWindow() != 0 }
}

/// 非 Windows：恒为 false。
#[cfg(not(windows))]
pub fn has_console() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只验证调用本身不会炸、且可重复调用。
    ///
    /// 真实隐藏效果必须在真机上看（测试进程通常没有控制台窗口）。
    #[test]
    fn hide_is_safe_to_call_repeatedly() {
        let first = hide();
        let second = hide();
        // 第一次藏掉之后第二次就没有窗口可藏了 —— 除非测试进程本来就没有控制台
        assert!(!second || first || !has_console());
        // 调用不该让程序处于"有窗口"的状态
        assert!(!has_console(), "hide 之后不应还附着控制台");
    }
}
