//! 系统托盘图标（Win32 `Shell_NotifyIconW`）。
//!
//! 设计（策划书 4.5）：**一个小图标，无气泡、无弹窗**，右键菜单提供：
//! 状态 / 立即停止录制 / 打开日志 / 打开数据目录 / 退出。
//!
//! 实现要点：
//! - 图标由 GDI 现场绘制（1616 圆角方块），**不依赖外部 .ico 文件**，
//!   省掉一个资源文件，也让部署更简单；
//! - 托盘需要一个窗口接收回调消息，这里创建一个**不可见的消息窗口**；
//! - 右击弹出原生菜单（`TrackPopupMenu`），点击后把命令通过 channel 送回守护进程；
//! - 全程不弹气泡通知（`NIF_INFO` 从未使用），符合「无打扰」要求。

// 说明：`overlay` 模块也声明了 `GetMessageW` / `TranslateMessage` 等同名 Win32 函数，
// 二者参数类型在 Rust 里是**不同的名义类型**（各自的 `Msg` / `Point`），
// 但内存布局与 ABI 完全一致（见本文件与 overlay.rs 的结构体定义）。
// 该 lint 只针对名义类型不同，这里显式说明并抑制。
#![allow(clashing_extern_declarations)]

use std::sync::mpsc;

type Hwnd = isize;
type Hinstance = isize;
type Hicon = isize;
type Hmenu = isize;
type Hdc = isize;
type Hbitmap = isize;
type Handle = isize;
type Bool = i32;
type Lpcwstr = *const u16;
type Uint = u32;
type Wparam = usize;
type Lparam = isize;
type Lresult = isize;

const WM_DESTROY: u32 = 0x0002;
const WM_CLOSE: u32 = 0x0010;
const WM_COMMAND: u32 = 0x0111;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_LBUTTONDBLCLK: u32 = 0x0203;
const WM_CONTEXTMENU: u32 = 0x007B;

/// 托盘回调消息（`WM_APP + 100`）
const WM_TRAY: u32 = 0x8000 + 100;

const NIM_ADD: u32 = 0;
const NIM_DELETE: u32 = 2;
const NIF_MESSAGE: u32 = 0x01;
const NIF_ICON: u32 = 0x02;
const NIF_TIP: u32 = 0x04;

const MF_STRING: u32 = 0x0000;
const MF_SEPARATOR: u32 = 0x0800;
const TPM_RIGHTBUTTON: u32 = 0x0002;
const TPM_RETURNCMD: u32 = 0x0100;
const TPM_NONOTIFY: u32 = 0x0080;

const ID_STOP: u32 = 1001;
const ID_LOGS: u32 = 1002;
const ID_DATA: u32 = 1003;
const ID_QUIT: u32 = 1004;

const WS_POPUP: u32 = 0x8000_0000;
const HWND_MESSAGE: Hwnd = -3;

#[repr(C)]
#[derive(Clone, Copy)]
struct NotifyIconDataW {
    cb_size: u32,
    h_wnd: Hwnd,
    u_id: u32,
    u_flags: u32,
    u_callback_message: u32,
    h_icon: Hicon,
    sz_tip: [u16; 128],
    dw_state: u32,
    /// `dwStateMask`  字段顺序必须与 Win32 完全一致，
    /// 少一个字段整个结构体就会偏移错位，Shell_NotifyIconW 会因 cbSize 不符而失败。
    dw_state_mask: u32,
    sz_info: [u16; 256],
    u_timeout_or_version: u32,
    sz_info_title: [u16; 64],
    dw_info_flags: u32,
    guid_item: [u8; 16],
    h_balloon_icon: Hicon,
}

impl Default for NotifyIconDataW {
    fn default() -> Self {
        // 手写 Default：数组长度超过 32 时标准库不提供 Default 实现
        // SAFETY: 全零对该 POD 结构体是合法初值（所有字段都是整数或定长数组）。
        unsafe { core::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct WndClassW {
    style: Uint,
    lpfn_wnd_proc: Option<unsafe extern "system" fn(Hwnd, Uint, Wparam, Lparam) -> Lresult>,
    cb_cls_extra: i32,
    cb_wnd_extra: i32,
    h_instance: Hinstance,
    h_icon: Hicon,
    h_cursor: Handle,
    hbr_background: Handle,
    lpsz_menu_name: Lpcwstr,
    lpsz_class_name: Lpcwstr,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Msg {
    hwnd: Hwnd,
    message: Uint,
    w_param: Wparam,
    l_param: Lparam,
    time: u32,
    pt_x: i32,
    pt_y: i32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct IconInfo {
    f_icon: Bool,
    x_hotspot: u32,
    y_hotspot: u32,
    hbm_mask: Hbitmap,
    hbm_color: Hbitmap,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_pels_per_meter: i32,
    y_pels_per_meter: i32,
    clr_used: u32,
    clr_important: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct RgbQuad {
    blue: u8,
    green: u8,
    red: u8,
    reserved: u8,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct BitmapInfo {
    header: BitmapInfoHeader,
    colors: [RgbQuad; 1],
}

#[link(name = "shell32")]
extern "system" {
    fn Shell_NotifyIconW(msg: u32, data: *mut NotifyIconDataW) -> Bool;
}

#[link(name = "user32")]
extern "system" {
    fn RegisterClassW(c: *const WndClassW) -> u16;
    fn CreateWindowExW(
        ex: Uint,
        class: Lpcwstr,
        title: Lpcwstr,
        style: Uint,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Hwnd,
        menu: Hmenu,
        inst: Hinstance,
        param: *mut core::ffi::c_void,
    ) -> Hwnd;
    fn DefWindowProcW(h: Hwnd, m: Uint, w: Wparam, l: Lparam) -> Lresult;
    fn DestroyWindow(h: Hwnd) -> Bool;
    fn PostQuitMessage(code: i32);
    fn GetMessageW(msg: *mut Msg, h: Hwnd, min: Uint, max: Uint) -> Bool;
    fn TranslateMessage(msg: *const Msg) -> Bool;
    fn DispatchMessageW(msg: *const Msg) -> Lresult;
    fn GetModuleHandleW(name: Lpcwstr) -> Hinstance;
    fn GetCursorPos(p: *mut Point) -> Bool;
    fn SetForegroundWindow(h: Hwnd) -> Bool;
    fn CreatePopupMenu() -> Hmenu;
    fn DestroyMenu(m: Hmenu) -> Bool;
    fn AppendMenuW(m: Hmenu, flags: Uint, id: usize, item: Lpcwstr) -> Bool;
    fn TrackPopupMenu(
        m: Hmenu,
        flags: Uint,
        x: i32,
        y: i32,
        reserved: i32,
        hwnd: Hwnd,
        rect: *const core::ffi::c_void,
    ) -> Bool;
    fn PostMessageW(h: Hwnd, msg: Uint, w: Wparam, l: Lparam) -> Bool;
    fn LoadIconW(inst: Hinstance, name: Lpcwstr) -> Hicon;
    fn DestroyIcon(i: Hicon) -> Bool;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateCompatibleDC(hdc: Hdc) -> Hdc;
    fn DeleteDC(hdc: Hdc) -> Bool;
    fn CreateDIBSection(
        hdc: Hdc,
        bmi: *const BitmapInfo,
        usage: u32,
        bits: *mut *mut core::ffi::c_void,
        section: isize,
        offset: u32,
    ) -> Hbitmap;
    fn CreateBitmap(
        w: i32,
        h: i32,
        planes: u32,
        bpp: u32,
        bits: *const core::ffi::c_void,
    ) -> Hbitmap;
    fn SelectObject(hdc: Hdc, obj: Handle) -> Handle;
    fn DeleteObject(obj: Handle) -> Bool;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_fixed(s: &str, n: usize) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().take(n.saturating_sub(1)).collect();
    v.resize(n, 0);
    v
}

/// 托盘命令（由菜单点击产生）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    /// 立即停止录制
    StopRecording,
    /// 打开日志目录
    OpenLogs,
    /// 打开数据目录
    OpenDataDir,
    /// 退出程序
    Quit,
}

/// 托盘句柄。
pub struct Tray {
    hwnd: Hwnd,
    rx: mpsc::Receiver<TrayCommand>,
}

impl Tray {
    /// 非阻塞取出一个命令。
    pub fn poll(&self) -> Option<TrayCommand> {
        self.rx.try_recv().ok()
    }

    /// 是否仍然有效。
    pub fn alive(&self) -> bool {
        self.hwnd != 0
    }

    /// 主动关闭。
    pub fn close(&self) {
        if self.hwnd != 0 {
            // SAFETY: 只投递 WM_CLOSE，不做解引用。
            unsafe {
                PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
            }
        }
    }
}

/// 用 GDI 现场画一个 1616 的圆角方块图标（深蓝底 + 白色中心点）。
fn make_icon() -> Hicon {
    const SZ: i32 = 16;
    // SAFETY: 所有句柄在本函数内创建并在返回前释放；像素缓冲按尺寸精确分配。
    unsafe {
        let screen = {
            extern "system" {
                fn GetDC(h: Hwnd) -> Hdc;
            }
            GetDC(0)
        };
        let dc = CreateCompatibleDC(screen);

        let mut bmi = BitmapInfo::default();
        bmi.header.size = std::mem::size_of::<BitmapInfoHeader>() as u32;
        bmi.header.width = SZ;
        bmi.header.height = -SZ; // 自上而下
        bmi.header.planes = 1;
        bmi.header.bit_count = 32;

        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let color = CreateDIBSection(dc, &bmi, 0, &mut bits, 0, 0);
        if color == 0 || bits.is_null() {
            let _ = DeleteDC(dc);
            return 0;
        }
        let old = SelectObject(dc, color);

        // 逐像素：圆角方块 + 中心白点（预乘 BGRA）
        let px = std::slice::from_raw_parts_mut(bits as *mut u8, (SZ * SZ * 4) as usize);
        let r = 3.0f32; // 圆角半径
        for y in 0..SZ {
            for x in 0..SZ {
                let fx = x as f32 + 0.5;
                let fy = y as f32 + 0.5;
                // 圆角矩形覆盖率：四角做简单距离判断
                let dx = (fx - r).max(0.0).max(r - fx).max(0.0);
                let _ = dx;
                let cx = (fx - SZ as f32 / 2.0).abs() - (SZ as f32 / 2.0 - r);
                let cy = (fy - SZ as f32 / 2.0).abs() - (SZ as f32 / 2.0 - r);
                let qx = cx.max(0.0);
                let qy = cy.max(0.0);
                let sdf = (qx * qx + qy * qy).sqrt() + cx.max(cy).min(0.0) - r;
                let cov = (0.5 - sdf).clamp(0.0, 1.0);
                if cov <= 0.0 {
                    continue;
                }
                // 中心白点
                let d = ((fx - 8.0).powi(2) + (fy - 8.0).powi(2)).sqrt();
                let (b, g, rr) = if d < 3.0 {
                    (255.0, 255.0, 255.0)
                } else {
                    (235.0, 111.0, 31.0)
                };
                let off = ((y * SZ + x) * 4) as usize;
                px[off] = (b * cov) as u8;
                px[off + 1] = (g * cov) as u8;
                px[off + 2] = (rr * cov) as u8;
                px[off + 3] = (cov * 255.0) as u8;
            }
        }
        SelectObject(dc, old);
        let _ = DeleteDC(dc);

        // 掩码位图全 0（不透明），32 位图标靠 alpha 通道
        let mask = CreateBitmap(SZ, SZ, 1, 1, std::ptr::null());

        let mut ii = IconInfo {
            f_icon: 1,
            x_hotspot: 0,
            y_hotspot: 0,
            hbm_mask: mask,
            hbm_color: color,
        };

        extern "system" {
            fn CreateIconIndirect(ii: *mut IconInfo) -> Hicon;
        }
        let icon = CreateIconIndirect(&mut ii);
        let _ = DeleteObject(color);
        let _ = DeleteObject(mask);
        icon
    }
}

/// 创建托盘图标。返回句柄与线程 JoinHandle。
pub fn spawn(tooltip: &str) -> std::io::Result<(Tray, std::thread::JoinHandle<()>)> {
    let (tx_ready, rx_ready) = mpsc::channel::<Result<Hwnd, String>>();
    let (tx_cmd, rx_cmd) = mpsc::channel::<TrayCommand>();
    let tip = tooltip.to_string();

    let join = std::thread::Builder::new()
        .name("vca-tray".to_string())
        .spawn(move || {
            // SAFETY: 标准 Win32 窗口 + 托盘创建流程；字符串缓冲区在调用期间存活。
            unsafe {
                let inst = GetModuleHandleW(core::ptr::null());
                let class = wide("VcaTrayWnd");
                let wc = WndClassW {
                    style: 0,
                    lpfn_wnd_proc: Some(wnd_proc),
                    cb_cls_extra: 0,
                    cb_wnd_extra: 0,
                    h_instance: inst,
                    h_icon: 0,
                    h_cursor: 0,
                    hbr_background: 0,
                    lpsz_menu_name: core::ptr::null(),
                    lpsz_class_name: class.as_ptr(),
                };
                if RegisterClassW(&wc) == 0 {
                    let _ = tx_ready.send(Err("RegisterClassW 失败".into()));
                    return;
                }
                // 只收消息的窗口（HWND_MESSAGE）
                let hwnd = CreateWindowExW(
                    0,
                    class.as_ptr(),
                    wide("vca-tray").as_ptr(),
                    WS_POPUP,
                    0,
                    0,
                    0,
                    0,
                    HWND_MESSAGE,
                    0,
                    inst,
                    core::ptr::null_mut(),
                );
                if hwnd == 0 {
                    let _ = tx_ready.send(Err("CreateWindowExW 失败".into()));
                    return;
                }

                // 把发送端存到窗口用户数据里，供 wnd_proc 使用
                let boxed = Box::into_raw(Box::new(tx_cmd));
                extern "system" {
                    fn SetWindowLongPtrW(h: Hwnd, idx: i32, v: isize) -> isize;
                }
                SetWindowLongPtrW(hwnd, -21, boxed as isize);

                let icon = {
                    let ic = make_icon();
                    if ic != 0 {
                        ic
                    } else {
                        LoadIconW(0, 32512 as Lpcwstr)
                    } // IDI_APPLICATION
                };

                let mut tip_buf = [0u16; 128];
                tip_buf.copy_from_slice(&wide_fixed(&tip, 128));
                let mut nid = NotifyIconDataW {
                    cb_size: std::mem::size_of::<NotifyIconDataW>() as u32,
                    h_wnd: hwnd,
                    u_id: 1,
                    u_flags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
                    u_callback_message: WM_TRAY,
                    h_icon: icon,
                    sz_tip: tip_buf,
                    ..Default::default()
                };

                if Shell_NotifyIconW(NIM_ADD, &mut nid) == 0 {
                    let _ = tx_ready.send(Err("Shell_NotifyIconW(NIM_ADD) 失败".into()));
                    return;
                }
                let _ = tx_ready.send(Ok(hwnd));

                // 消息循环
                let mut msg: Msg = core::mem::zeroed();
                while GetMessageW(&mut msg, 0, 0, 0) > 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }

                // 清理
                let _ = Shell_NotifyIconW(NIM_DELETE, &mut nid);
                if icon != 0 {
                    let _ = DestroyIcon(icon);
                }
            }
        })?;

    match rx_ready.recv() {
        Ok(Ok(hwnd)) => Ok((Tray { hwnd, rx: rx_cmd }, join)),
        Ok(Err(e)) => Err(std::io::Error::other(e)),
        Err(_) => Err(std::io::Error::other("托盘线程提前退出")),
    }
}

unsafe extern "system" fn wnd_proc(hwnd: Hwnd, msg: Uint, w: Wparam, l: Lparam) -> Lresult {
    match msg {
        WM_TRAY => {
            let ev = (l as u32) & 0xFFFF;
            if ev == WM_RBUTTONUP || ev == WM_CONTEXTMENU {
                show_menu(hwnd);
            } else if ev == WM_LBUTTONDBLCLK {
                send_cmd(hwnd, TrayCommand::OpenDataDir);
            }
            0
        }
        WM_COMMAND => {
            let id = (w & 0xFFFF) as u32;
            let cmd = match id {
                ID_STOP => Some(TrayCommand::StopRecording),
                ID_LOGS => Some(TrayCommand::OpenLogs),
                ID_DATA => Some(TrayCommand::OpenDataDir),
                ID_QUIT => Some(TrayCommand::Quit),
                _ => None,
            };
            if let Some(c) = cmd {
                send_cmd(hwnd, c);
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            extern "system" {
                fn GetWindowLongPtrW(h: Hwnd, idx: i32) -> isize;
            }
            let p = GetWindowLongPtrW(hwnd, -21);
            if p != 0 {
                drop(Box::from_raw(p as *mut mpsc::Sender<TrayCommand>));
            }
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, w, l),
    }
}

/// 弹出右键菜单并把选择结果发回守护进程。
unsafe fn show_menu(hwnd: Hwnd) {
    let menu = CreatePopupMenu();
    if menu == 0 {
        return;
    }
    let items = [
        (ID_STOP, "立即停止录制"),
        (0, ""),
        (ID_LOGS, "打开日志"),
        (ID_DATA, "打开数据目录"),
        (0, ""),
        (ID_QUIT, "退出"),
    ];
    for (id, text) in items {
        if id == 0 {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, core::ptr::null());
        } else {
            let t = wide(text);
            let _ = AppendMenuW(menu, MF_STRING, id as usize, t.as_ptr());
        }
    }

    let mut pt = Point::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(hwnd);
    let _ = TrackPopupMenu(
        menu,
        TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
        pt.x,
        pt.y,
        0,
        hwnd,
        core::ptr::null(),
    );
    let _ = DestroyMenu(menu);
}

/// 把命令通过窗口用户数据里的 channel 发出去。
unsafe fn send_cmd(hwnd: Hwnd, cmd: TrayCommand) {
    extern "system" {
        fn GetWindowLongPtrW(h: Hwnd, idx: i32) -> isize;
    }
    let p = GetWindowLongPtrW(hwnd, -21);
    if p != 0 {
        let tx = &*(p as *const mpsc::Sender<TrayCommand>);
        let _ = tx.send(cmd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_fixed_pads_and_terminates() {
        let v = wide_fixed("ab", 8);
        assert_eq!(v.len(), 8);
        assert_eq!(v[0], 'a' as u16);
        assert_eq!(v[1], 'b' as u16);
        assert_eq!(v[2], 0);
        assert_eq!(v[7], 0);
    }

    #[test]
    fn wide_fixed_truncates_long_text() {
        let v = wide_fixed("abcdefghij", 4);
        assert_eq!(v.len(), 4);
        assert_eq!(v[3], 0, "必须留一位给 NUL");
    }

    #[test]
    fn commands_are_distinct() {
        let a = TrayCommand::StopRecording;
        let b = TrayCommand::Quit;
        assert_ne!(a, b);
        assert_eq!(a, TrayCommand::StopRecording);
    }

    #[test]
    fn notify_icon_data_size_is_sane() {
        // 结构体布局必须与 Win32 期望一致（NOTIFYICONDATAW 在 64 位下约 976 字节）
        // 与 Win32 的 NOTIFYICONDATAW（Vista+）必须**逐字节一致**，
        // 否则 Shell_NotifyIconW 会因 cbSize 不符而失败。
        let n = std::mem::size_of::<NotifyIconDataW>();
        assert_eq!(n, 976, "NOTIFYICONDATAW 大小应为 976，实际 {n}");
    }
}
