//! 窗口查找与置前（Win32）。
//!
//! 界面是以 `--app=` 拉起来的独立 Edge 窗口，标题取自页面的 `<title>`。
//! 用户左键点托盘图标时，期望的是**把那个窗口调到前面**，而不是再开一个 ——
//! 所以需要按标题把它找回来。
//!
//! 放在平台层是因为上层 `vca-gui` 是 `#![forbid(unsafe_code)]`，
//! 而这里必须调 Win32。

/// 按窗口标题找窗口并置于前台。
///
/// 返回值：
/// - `true`：找到了，并且已经（尝试）显示 / 还原 / 置前。
/// - `false`：没找到（或当前不是 Windows），调用方应当新开一个窗口。
///
/// # 为什么要处理「最小化」和「隐藏」
///
/// 只调 `SetForegroundWindow` 是不够的：对一个最小化的窗口置前，用户看到的
/// 还是任务栏上那个没还原的按钮；对隐藏窗口更是毫无效果。所以先看状态：
/// 最小化就 `SW_RESTORE`（还原并激活），隐藏就 `SW_SHOW`。
pub fn focus_by_title(title: &str) -> bool {
    imp::focus_by_title(title)
}

#[cfg(windows)]
mod imp {
    type Hwnd = isize;
    type Bool = i32;

    const SW_RESTORE: i32 = 9;
    const SW_SHOW: i32 = 5;

    #[link(name = "user32")]
    extern "system" {
        fn FindWindowW(class: *const u16, title: *const u16) -> Hwnd;
        fn IsWindowVisible(h: Hwnd) -> Bool;
        fn IsIconic(h: Hwnd) -> Bool;
        fn ShowWindow(h: Hwnd, cmd: i32) -> Bool;
        fn SetForegroundWindow(h: Hwnd) -> Bool;
    }

    pub fn focus_by_title(title: &str) -> bool {
        let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();

        // SAFETY: 传的是本函数内构造、生命周期覆盖整个调用的宽字符串。
        // 拿到的 hwnd 只是一个标识，不持有所有权，不需要（也不能）释放。
        unsafe {
            // 第一个参数传 null：只按标题找，不限窗口类。
            let hwnd = FindWindowW(core::ptr::null(), wide.as_ptr());
            if hwnd == 0 {
                return false;
            }

            // 顺序要紧：先还原/显示，再置前。
            // 反过来的话，对最小化窗口的 SetForegroundWindow 基本无效 ——
            // 窗口确实"被激活"了，但还缩在任务栏里，用户看着就是没反应。
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            } else if IsWindowVisible(hwnd) == 0 {
                ShowWindow(hwnd, SW_SHOW);
            }
            SetForegroundWindow(hwnd);
            true
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn focus_by_title(_title: &str) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_window_returns_false_without_panicking() {
        // 用一个几乎不可能存在的标题：必须安静返回 false，不能崩。
        assert!(!focus_by_title("__VCA_不存在的窗口标题_请勿创建__"));
    }
}
