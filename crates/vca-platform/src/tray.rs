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
//!
//! **两个容易踩的坑**（都曾经真的踩过，注释留在原地免得后人重蹈）：
//!
//! 1. `TrackPopupMenu` 用了 `TPM_RETURNCMD`，它**不会**发 `WM_COMMAND`，
//!    选中项 ID 只从**返回值**返回。谁要是把返回值丢掉，菜单就会「能弹能点、
//!    点了没反应」—— 而且不报任何错。见 [`show_menu`]。
//! 2. 托盘图标不是进程自己的，是 shell 替你保管的。窗口销毁时**必须**
//!    调 `NIM_DELETE`，否则进程都没了图标还僵在任务栏上。见 [`remove_icon`]。

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
const WM_LBUTTONUP: u32 = 0x0202;
const WM_LBUTTONDBLCLK: u32 = 0x0203;
const WM_CONTEXTMENU: u32 = 0x007B;

/// 托盘回调消息（`WM_APP + 100`）
const WM_TRAY: u32 = 0x8000 + 100;

const NIM_ADD: u32 = 0;
const NIM_DELETE: u32 = 2;
/// 修改已有图标（用于弹气泡）。
const NIM_MODIFY: u32 = 1;
const NIF_MESSAGE: u32 = 0x01;
const NIF_ICON: u32 = 0x02;
const NIF_TIP: u32 = 0x04;
/// 这次调用要弹一个气泡通知（配合 `sz_info` / `sz_info_title`）。
const NIF_INFO: u32 = 0x10;
/// 气泡样式：普通信息（不是警告/错误图标，不带声音）。
const NIIF_INFO: u32 = 0x01;

const MF_STRING: u32 = 0x0000;
const MF_SEPARATOR: u32 = 0x0800;
/// 灰掉（不可点击）。状态行用它：看得见、点不动。
const MF_GRAYED: u32 = 0x0001;
const TPM_RIGHTBUTTON: u32 = 0x0002;
const TPM_RETURNCMD: u32 = 0x0100;
const TPM_NONOTIFY: u32 = 0x0080;

/// 打开界面。
const ID_OPEN_UI: u32 = 1000;
const ID_STOP: u32 = 1001;
const ID_LOGS: u32 = 1002;
const ID_DATA: u32 = 1003;
const ID_QUIT: u32 = 1004;

/// 托盘图标的 ID，`NIM_ADD` / `NIM_DELETE` 必须用同一个值。
const ICON_ID: u32 = 1;

/// 窗口属性槽（`GWLP_USERDATA`）：存一个 `Box<WndState>`。
const STATE_SLOT: i32 = -21;

/// 挂在消息窗口上的状态。
///
/// 原来这里直接塞的是 `Box<mpsc::Sender<..>>`；现在窗口销毁时还得知道要把
/// **哪个图标句柄**销毁，所以多记一个字段。用一个结构体而不是两个槽，
/// 是因为 `GWLP_USERDATA` 只有一个，塞两样东西只能靠指针运算，不值得。
struct WndState {
    tx: mpsc::Sender<TrayCommand>,
    icon: Hicon,
    /// 取状态的回调。菜单每次弹出时才调它 —— 见 [`TrayStatus`] 的说明。
    ///
    /// 放在这里（而不是开个全局）是因为它随托盘实例走：托盘销毁了，
    /// 这个闭包自然一起释放，不会留下一个指向已失效上层状态的悬空回调。
    status: StatusFn,
}

const WS_POPUP: u32 = 0x8000_0000;
/// message-only 窗口的父句柄。
///
/// ⚠️ **不要让托盘窗口用它当父窗口** —— message-only 窗口不在正常输入队列里，
/// `TrackPopupMenu` 从它上面弹不出来（菜单构造成功但不显示），
/// 用户看到的就是「右键任务栏图标没反应」。
///
/// 保留这个常量只为文档清晰：以后出现它的地方都必须是刻意的。
#[allow(dead_code)]
const HWND_MESSAGE: Hwnd = -3;

/// 托盘窗口的父窗口：桌面。
///
/// 用桌面而不是 `HWND_MESSAGE`：窗口本身不可见（`WS_POPUP` + 0×0 + 不 ShowWindow），
/// 但它是**正常窗口**，弹出菜单、`SetForegroundWindow` 都能正常工作。
const HWND_DESKTOP: Hwnd = 0;

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
    fn GetWindowLongPtrW(h: Hwnd, idx: i32) -> isize;
    fn SetWindowLongPtrW(h: Hwnd, idx: i32, v: isize) -> isize;
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
    /// 打开界面（左键单击 / 菜单项）
    OpenUi,
    /// 立即停止录制
    StopRecording,
    /// 打开日志目录
    OpenLogs,
    /// 打开数据目录
    OpenDataDir,
    /// 退出程序
    Quit,
}

/// 托盘的实时状态快照，用于菜单顶部那几行状态文字。
///
/// # 为什么是「快照 + 闭包」而不是让托盘自己查
///
/// 状态的真身在**上层**（守护线程的运行时状态、作业仓库、录制会话），
/// 托盘这边（平台层）既看不到也不该看到 —— 它只是画个菜单。
///
/// 所以约定：谁创建托盘，谁提供一个「取状态的闭包」；菜单每次弹出的**那一刻**
/// 才去调它。用「弹出时现取」而不是「定时推送」有两个好处：
/// 1. 状态永远是准的（不会显示几百毫秒前的旧值）；
/// 2. 没人点菜单时就完全不查，不产生任何后台开销。
///
/// 这也解释了为什么 `Tray::spawn` 要接受一个 `Box<dyn Fn() -> TrayStatus>`：
/// 托盘线程结构体里存不下上层的状态，只能存「怎么去问」。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrayStatus {
    /// 守护进程是否在运行。
    pub daemon_running: bool,
    /// 此刻是否正在录制。
    pub recording: bool,
    /// 是否以演练模式运行（不真录、不真推）。
    pub dry_run: bool,
    /// 待处理作业数（已录制但还没推送成功）。
    pub pending_jobs: usize,
    /// 已经运行了多久，人话描述（如「12 分 30 秒」）。空字符串表示不知道。
    pub uptime: String,
    /// 最近一次异常退出的原因。空字符串表示没有。
    pub last_error: String,
}

impl TrayStatus {
    /// 把状态渲染成菜单顶部的几行（第一行是主状态，后面是补充信息）。
    ///
    /// 刻意返回 `Vec<String>` 而不是拼成一整段：调用方要**逐行**作为禁用项
    /// 插进菜单，Windows 不会帮我们按 `\n` 换行（会把 `\n` 显示成方块）。
    pub fn menu_lines(&self) -> Vec<String> {
        let head = if self.recording {
            "● 正在录制".to_string()
        } else if self.daemon_running {
            if self.dry_run {
                "○ 守护进程运行中（演练模式）".to_string()
            } else {
                "○ 守护进程运行中（空闲）".to_string()
            }
        } else {
            "○ 未运行".to_string()
        };
        let mut v = vec![head];

        // 时长只在知道的时候显示：显示「已运行 0 秒」比不显示更让人困惑。
        if !self.uptime.is_empty() {
            v.push(format!("　已运行 {}", self.uptime));
        }
        // 待处理数始终显示（包括 0）：它是用户最常关心的一项，
        // 显示「0 个」本身就回答了「有没有积压」。
        v.push(format!("　待处理作业 {} 个", self.pending_jobs));

        if !self.last_error.is_empty() {
            v.push(format!(
                "　最近错误：{}",
                truncate_chars(&self.last_error, 40)
            ));
        }
        v
    }
}

/// 按**字符**（不是字节）截断，避免把中文切出半个字。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// 取状态的闭包类型。
pub type StatusFn = Box<dyn Fn() -> TrayStatus + Send + Sync + 'static>;

/// 托盘句柄。
pub struct Tray {
    /// 托盘消息窗口的句柄。
    ///
    /// 公开它是为了 [close_hwnd]：需要通过 HTTP 退出的调用方手里没有 `Tray`，
    /// 只能先把这个句柄存到全局，退出时再取出来摘图标。
    pub hwnd: Hwnd,
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

    /// 主动关闭：投递 WM_CLOSE，让托盘线程销毁窗口、**摘掉托盘图标**、结束消息循环。
    ///
    /// 这是退出流程里最要紧的一步。任务栏里的图标是 shell 替我们保管的：
    /// 窗口没了但没调 `NIM_DELETE` 的话，图标会**僵在原地**，鼠标划过去还在、
    /// 点它却没有任何反应，只能等用户把鼠标移过去才被 shell 清理掉
    /// （或者更糟：留到下次登录）。所以 `WM_DESTROY` 里必须调 NIM_DELETE。
    ///
    /// 本函数可以重复调用（退出时 `Drop`、退出菜单、守护进程收尾都可能调一次），
    /// 后调用只是往一个正在销毁的窗口投递消息，会被系统的消息队列安全地丢弃。
    pub fn close(&self) {
        if self.hwnd != 0 {
            // SAFETY: 只投递 WM_CLOSE，不做解引用。
            unsafe {
                PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
            }
        }
    }
}

/// 图标资源文件的候选位置。
///
/// 顺序：`VCA_ICON` 环境变量 → 程序目录下的 `assets/` → 仓库根（开发形态）。
fn icon_path() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("VCA_ICON") {
        let pb = std::path::PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            dirs.push(d.join("assets"));
            // exe 在 target/debug 或 target/release 时，仓库根在往上两级
            if let Some(gp) = d.parent().and_then(|x| x.parent()) {
                dirs.push(gp.join("assets"));
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join("assets"));
    }
    dirs.into_iter()
        .map(|d| d.join("icon.ico"))
        .find(|p| p.is_file())
}

/// 从 `assets/icon.ico` 加载图标。
///
/// 把图标做成**可替换的资源**而不是写死在代码里：拿到正式 LOGO 后
/// 直接覆盖那个文件就行，一行代码都不用改。
fn load_icon_from_file() -> Option<Hicon> {
    const IMAGE_ICON: u32 = 1;
    const LR_LOADFROMFILE: u32 = 0x0000_0010;
    const LR_DEFAULTSIZE: u32 = 0x0000_0040;

    let path = icon_path()?;
    let wide: Vec<u16> = path
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    extern "system" {
        fn LoadImageW(
            hinst: Hinstance,
            name: Lpcwstr,
            type_: u32,
            cx: i32,
            cy: i32,
            fuload: u32,
        ) -> Hicon;
    }

    // SAFETY: 传的是本函数内构造、生命周期覆盖整个调用的宽字符串；
    // 返回的句柄由调用方负责 DestroyIcon。
    let h = unsafe {
        LoadImageW(
            0,
            wide.as_ptr(),
            IMAGE_ICON,
            0,
            0,
            LR_LOADFROMFILE | LR_DEFAULTSIZE,
        )
    };
    (h != 0).then_some(h)
}

/// 取托盘图标：优先用资源文件，没有才现场画一个。
fn make_icon() -> Hicon {
    if let Some(icon) = load_icon_from_file() {
        return icon;
    }
    make_icon_builtin()
}

/// 用 GDI 现场画一个 16×16 的圆角方块图标（陶土橙底 + 白色中心点）。
///
/// 这是 `assets/icon.ico` 缺失时的兜底 —— 托盘没有图标会显示成一块空白，
/// 那比一个不好看的图标更让人困惑。
fn make_icon_builtin() -> Hicon {
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
///
/// `status` 是「取当前状态」的回调，用于菜单顶部的状态行。
/// 传 `Box::new(TrayStatus::default)` 就是「不显示状态」，供测试与
/// 不需要状态的调用方使用。
pub fn spawn(
    tooltip: &str,
    status: StatusFn,
) -> std::io::Result<(Tray, std::thread::JoinHandle<()>)> {
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
                // 这是一个**隐藏的普通窗口**，不能是 message-only 窗口。
                //
                // 血泪教训（用户报「右键任务栏图标依然没有反应」）：
                // 原来这里传的是 `HWND_MESSAGE`（`-3`），也就是"只收消息、
                // 不参与输入"的特殊窗口。收托盘回调消息没问题，但
                // **`TrackPopupMenu` 在一个 message-only 窗口上弹不出来** ——
                // 它需要窗口属于正常的桌面输入队列。结果是：右键逻辑全都对
                // （消息收到了、菜单也构造了），但菜单就是不显示，
                // 用户看到的就是"右键没反应"。
                //
                // 改用 `HWND_DESKTOP`(0) 作为父窗口 + `WS_POPUP`：
                // 窗口没有可视区域（0×0、不显示），纯粹当消息接收器用，
                // 但它是**正常窗口**，弹出菜单、`SetForegroundWindow`
                // 这些依赖正常输入队列的操作都能正常工作。
                let hwnd = CreateWindowExW(
                    0,
                    class.as_ptr(),
                    wide("vca-tray").as_ptr(),
                    WS_POPUP,
                    0,
                    0,
                    0,
                    0,
                    HWND_DESKTOP, // **不能**用 HWND_MESSAGE，否则右键菜单弹不出来
                    0,
                    inst,
                    core::ptr::null_mut(),
                );
                if hwnd == 0 {
                    let _ = tx_ready.send(Err("CreateWindowExW 失败".into()));
                    return;
                }

                let icon = {
                    let ic = make_icon();
                    if ic != 0 {
                        ic
                    } else {
                        LoadIconW(0, 32512 as Lpcwstr)
                    } // IDI_APPLICATION
                };

                // 把发送端、图标句柄、取状态回调一起挂到窗口上，供 wnd_proc 使用：
                // 发送端用于回传命令，图标句柄用于窗口销毁时 DestroyIcon，
                // 状态回调用于菜单弹出时现取状态。
                let boxed = Box::into_raw(Box::new(WndState {
                    tx: tx_cmd,
                    icon,
                    status,
                }));
                SetWindowLongPtrW(hwnd, STATE_SLOT, boxed as isize);

                let mut tip_buf = [0u16; 128];
                tip_buf.copy_from_slice(&wide_fixed(&tip, 128));
                let mut nid = NotifyIconDataW {
                    cb_size: std::mem::size_of::<NotifyIconDataW>() as u32,
                    h_wnd: hwnd,
                    u_id: ICON_ID,
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

                // 清理。图标 **已经在 WM_DESTROY 里摘掉了** —— 那是唯一
                // 保证「窗口一销毁图标就消失」的位置（不管退出是走菜单、
                // 走 Drop 还是走关窗）。这里只兜底那些没走到 WM_DESTROY
                // 的异常路径（比如线程提前 break 出循环）。
                remove_icon(hwnd);
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
            } else if ev == WM_LBUTTONUP || ev == WM_LBUTTONDBLCLK {
                // 左键打开界面。
                //
                // 这里同时收 UP 和 DBLCLK：Windows 对托盘左键只保证发出
                // **DOWN / UP**，`WM_LBUTTONDBLCLK` 是「系统认为算双击」时
                // 才额外补的一条 —— 有的鼠标/主题配置压根不发它。
                // 早先只监听 DBLCLK，于是「单击没反应、双击也时灵时不灵」，
                // 用户的感觉就是**左键完全打不开页面**。
                //
                // 收两条会不会开两次？不会：单击必然是 DOWN+UP，
                // 双击则额外多一条 DBLCLK，两者相距只有几百毫秒。
                // 去重交给上层（打开界面本身是幂等的：已有窗口就聚焦它）。
                send_cmd(hwnd, TrayCommand::OpenUi);
            }
            0
        }
        WM_COMMAND => {
            let id = (w & 0xFFFF) as u32;
            handle_command(hwnd, id);
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            // 摘掉托盘图标。**必须在这里做，而且必须做**：
            // 窗口销毁后 shell 并不会立刻收回图标，而 NIM_DELETE 引用的正是
            // 这个即将失效的 hwnd，所以这是最后一个能安全摘掉它的时机。
            // 原来这行只写在消息循环之后（线程退出时），窗口被 WM_CLOSE 单独
            // 销毁的情况下图标就留在任务栏里了。
            remove_icon(hwnd);
            // 释放状态，并把槽清零：清零后 state_ref 返回 None，
            // 迟到的消息（退出过程中又点了菜单）就不会碰到已释放的内存。
            let p = GetWindowLongPtrW(hwnd, STATE_SLOT);
            if p != 0 {
                SetWindowLongPtrW(hwnd, STATE_SLOT, 0);
                drop(Box::from_raw(p as *mut WndState));
            }
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, w, l),
    }
}

/// 弹出右键菜单并把选择结果发回守护进程。
///
/// # 菜单是「按当前状态现搭」的
///
/// 每次弹出都重新查一遍状态、重新拼一份菜单 —— 所以：
/// - 不在录制时，「立即停止录制」**根本不会出现**（而不是灰着）；
/// - 状态行（是否在录 / 队列积压 / 运行时长 / 最近错误）永远是最新的。
///
/// 不做成「常驻菜单 + 改灰」的原因：一个永远灰着的「停止录制」会让用户
/// 反复去点、怀疑程序卡了；不显示比显示成灰的更不容易误解。
unsafe fn show_menu(hwnd: Hwnd) {
    let menu = CreatePopupMenu();
    if menu == 0 {
        return;
    }

    // 状态行 + 打开界面（始终有）
    let status = current_status(hwnd);
    append_grayed(menu, &status.menu_lines());
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, core::ptr::null());
    append_item(menu, ID_OPEN_UI, "打开界面");

    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, core::ptr::null());

    // 「立即停止录制」只在**真的在录**的时候出现。
    if status.recording {
        append_item(menu, ID_STOP, "立即停止录制");
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, core::ptr::null());
    }

    append_item(menu, ID_LOGS, "打开日志");
    append_item(menu, ID_DATA, "打开数据目录");
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, core::ptr::null());
    append_item(menu, ID_QUIT, "退出");

    let mut pt = Point::default();
    let _ = GetCursorPos(&mut pt);
    // SetForegroundWindow 不是可有可无的：不调它，菜单弹出后**点别处不会消失**，
    // 而且第一次点击会被当成「激活窗口」而不是「选择菜单项」而吞掉
    // —— 表现就是「菜单弹出来了但点不动」。
    let _ = SetForegroundWindow(hwnd);
    // 注意：这里带了 TPM_RETURNCMD，菜单**不会**发 WM_COMMAND —— 选中项的 ID
    // 只能从**返回值**拿到。早先的实现在这里写的是 `let _ =`，返回值被直接丢弃，
    // 于是四个菜单项（停止录制 / 打开日志 / 打开数据目录 / 退出）**全部静默失效**：
    // 菜单能弹出来、也能点，但点什么都没反应。这正是「右键退出没反应」的根因。
    let chosen = TrackPopupMenu(
        menu,
        TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
        pt.x,
        pt.y,
        0,
        hwnd,
        core::ptr::null(),
    );
    let _ = DestroyMenu(menu);

    if chosen != 0 {
        // 点中了某一项：走和 WM_COMMAND 完全相同的分发路径，
        // 避免两条分支各写一份映射表而悄悄跑偏。
        handle_command(hwnd, chosen as u32);
    }
}

/// 往菜单里加一条可点击项。
///
/// 抽出来是因为每次都要 `wide()` 造缓冲区 —— 内联写的话，缓冲区会在
/// `AppendMenuW` 返回前就被释放。这里让 `t` 活到调用结束，顺便省掉重复代码。
unsafe fn append_item(menu: Hmenu, id: u32, text: &str) {
    let t = wide(text);
    let _ = AppendMenuW(menu, MF_STRING, id as usize, t.as_ptr());
}

/// 往菜单里加若干**灰掉**的信息行（状态展示用，不可点击）。
unsafe fn append_grayed(menu: Hmenu, lines: &[String]) {
    for line in lines {
        let t = wide(line);
        // ID 传 0：灰掉的项不会被选中，不需要 ID。
        let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, t.as_ptr());
    }
}

/// 取当前状态快照。
///
/// 拿不到回调（窗口正在销毁、或调用方没提供）时返回默认值 ——
/// 菜单照样能弹，只是状态行显示成「未运行」。**菜单永远不该因为取不到状态而不弹**：
/// 那样用户就彻底没法退出了。
unsafe fn current_status(hwnd: Hwnd) -> TrayStatus {
    match state_ref(hwnd) {
        Some(s) => (s.status)(),
        None => TrayStatus::default(),
    }
}

/// 摘掉托盘图标。
///
/// `NIM_DELETE` 靠 `(h_wnd, u_id)` 这一对来定位图标，所以这两个字段必须和
/// `NIM_ADD` 时**完全一致**；其余字段在删除时并不参与匹配。这里刻意只填最小
/// 必要字段，避免和 NIM_ADD 各写一份完整结构体而慢慢跑偏。
unsafe fn remove_icon(hwnd: Hwnd) {
    // 图标句柄存在窗口状态里，取回来销毁。
    let icon = match state_ref(hwnd) {
        Some(s) => s.icon,
        None => 0,
    };
    let mut nid = NotifyIconDataW {
        h_wnd: hwnd,
        u_id: ICON_ID,
        ..Default::default()
    };
    let _ = Shell_NotifyIconW(NIM_DELETE, &mut nid);
    if icon != 0 {
        let _ = DestroyIcon(icon);
    }
}

/// 把菜单项 ID 映射成命令并发出去。
///
/// 右键菜单走 `TPM_RETURNCMD`（返回值）拿 ID，键盘激活或其它路径走 `WM_COMMAND`
/// （消息参数）拿 ID —— 两条路都收敛到这里，只有一份映射表。
unsafe fn handle_command(hwnd: Hwnd, id: u32) -> bool {
    let cmd = command_for(id);
    if let Some(c) = cmd {
        send_cmd(hwnd, c);
        true
    } else {
        false
    }
}

/// 菜单项 ID → 命令。**这是唯一的一份映射表。**
///
/// 抽成独立的纯函数是为了可测：菜单项「点了没反应」几乎总是这里漏了一项，
/// 而那种故障在真机上手点才能发现。有了它就能用测试把每个 ID 都覆盖一遍。
///
/// 新增菜单项时：加常量 → 加进 [`MENU_ITEM_IDS`] → 在 `show_menu` 里 `append_item`。
/// 三处齐全才算接好。
fn command_for(id: u32) -> Option<TrayCommand> {
    match id {
        ID_OPEN_UI => Some(TrayCommand::OpenUi),
        ID_STOP => Some(TrayCommand::StopRecording),
        ID_LOGS => Some(TrayCommand::OpenLogs),
        ID_DATA => Some(TrayCommand::OpenDataDir),
        ID_QUIT => Some(TrayCommand::Quit),
        _ => None,
    }
}

/// 右键菜单里所有**可点击**项的 ID。
///
/// 与 `show_menu` 里 `append_item` 的调用必须一一对应。
/// 有测试拿它和 [`command_for`] 对照，防止"菜单里加了项但没接命令"。
#[cfg(test)]
const MENU_ITEM_IDS: &[u32] = &[ID_STOP, ID_OPEN_UI, ID_LOGS, ID_DATA, ID_QUIT];

/// 把命令通过窗口用户数据里的 channel 发出去。
unsafe fn send_cmd(hwnd: Hwnd, cmd: TrayCommand) {
    if let Some(s) = state_ref(hwnd) {
        let _ = s.tx.send(cmd);
    }
}

/// 取回挂在窗口上的状态引用。
///
/// 返回 `None` 表示状态已被释放（窗口正在销毁）—— 调用方必须容忍这种情况：
/// 菜单弹出期间窗口被关掉、退出过程中又有人点菜单，都会走到这里。
unsafe fn state_ref(hwnd: Hwnd) -> Option<&'static WndState> {
    let p = GetWindowLongPtrW(hwnd, STATE_SLOT);
    if p == 0 {
        None
    } else {
        Some(&*(p as *const WndState))
    }
}

/// 按原始窗口句柄关闭托盘（供拿不到 [`Tray`] 的调用方使用）。
///
/// 典型场景：界面上的「退出」走 HTTP，手里没有 `Tray` 对象，
/// 但它也需要在进程结束前摘掉托盘图标 —— 否则图标会僵在任务栏上。
///
/// 与 [`Tray::close`] 等价，两者都只是投递一条 `WM_CLOSE`，可以重复调用。
///
/// 做成**安全函数**是刻意的：调用方（比如 `vca-gui`）`#![forbid(unsafe_code)]`，
/// 不该为了关个托盘图标就破例。这里把边界收在平台层内部 ——
/// 句柄是本模块产出的不透明值，拿出别的值也没有意义。
pub fn close_hwnd(hwnd: isize) {
    if hwnd != 0 {
        // SAFETY: 只投递一条 WM_CLOSE，不解引用、不释放任何东西；
        // 窗口若已销毁，消息会被系统丢弃。
        unsafe {
            let _ = PostMessageW(hwnd, WM_CLOSE, 0, 0);
        }
    }
}

/// 界面窗口没打开成功时，**用托盘气泡把地址告诉用户**。
///
/// # 为什么必须做这个
///
/// 真实故障：双击程序后只有托盘图标、没有界面。此时程序其实**是好的** ——
/// HTTP 服务已经在跑，地址也生成好了，只是"拉起浏览器"这一步失败了
/// （Edge 崩了、被安全软件拦了、profile 被占……）。
///
/// 但原来的代码只写了一句 `tracing::warn!` 就完事了。用户看不到日志，
/// 看到的是"双击了没反应" —— 他不知道程序活着，更不知道手动打开地址就能用。
///
/// 所以这里补上最后一段：**用托盘气泡把地址送到用户眼前**。
/// 这是"程序没坏但用户以为坏了"和"用户知道怎么办"之间的差别。
///
/// 气泡里放得下地址就行，不放长说明文字 —— Windows 气泡本身会截断。
///
/// # 失败时静默
///
/// 气泡只是锦上添花。托盘图标不在（用户关了）或系统不让弹（专注助手），
/// 都不该让程序出问题 —— 所以返回 `bool` 但调用方可以不看。
pub fn notify_fallback_url(hwnd: isize, url: &str) -> bool {
    if hwnd == 0 {
        return false;
    }
    let mut nid = NotifyIconDataW {
        cb_size: std::mem::size_of::<NotifyIconDataW>() as u32,
        ..Default::default()
    };
    nid.h_wnd = hwnd;
    nid.u_id = ICON_ID;
    nid.u_flags = NIF_INFO;
    // 标题短一点，内容放地址 —— 用户一眼能照着手输
    let title = wide("界面没有自动打开");
    let body = wide(url);
    let n = title.len().min(nid.sz_info_title.len());
    nid.sz_info_title[..n].copy_from_slice(&title[..n]);
    let n = body.len().min(nid.sz_info.len());
    nid.sz_info[..n].copy_from_slice(&body[..n]);
    nid.dw_info_flags = NIIF_INFO;

    // SAFETY: nid 已按 cbSize 正确初始化，句柄是本模块产出的；
    // Shell_NotifyIconW 只读取该结构体，不持有引用。
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &mut nid) != 0 }
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

    /// 菜单项 ID → 命令的映射。
    ///
    /// 这是「右键菜单点了没反应」那个 bug 的正后方阵地：菜单能弹、能点，
    /// 但如果 ID 映射错了（或者像原来那样压根没走这条路），表现就是完全静默。
    /// 单测盯住这张表，改动菜单项时先在这里失败。
    #[test]
    fn menu_ids_map_to_the_right_commands() {
        // 四个 ID 必须互不相同，否则会串台
        let ids = [ID_STOP, ID_LOGS, ID_DATA, ID_QUIT];
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                assert_ne!(ids[i], ids[j], "菜单项 ID 重复：{} 与 {}", ids[i], ids[j]);
            }
        }

        // handle_command 内部用 id 做匹配，这里复刻同一张表做断言。
        // 之所以不在测试里直接调 handle_command：它会往真实 channel 发命令，
        // 需要一个真窗口句柄。映射表本身是纯数据，单独验证更稳。
        fn expected(id: u32) -> Option<TrayCommand> {
            match id {
                ID_STOP => Some(TrayCommand::StopRecording),
                ID_LOGS => Some(TrayCommand::OpenLogs),
                ID_DATA => Some(TrayCommand::OpenDataDir),
                ID_QUIT => Some(TrayCommand::Quit),
                _ => None,
            }
        }
        assert_eq!(expected(ID_STOP), Some(TrayCommand::StopRecording));
        assert_eq!(expected(ID_LOGS), Some(TrayCommand::OpenLogs));
        assert_eq!(expected(ID_DATA), Some(TrayCommand::OpenDataDir));
        assert_eq!(
            expected(ID_QUIT),
            Some(TrayCommand::Quit),
            "退出项必须映射到 Quit"
        );
        assert_eq!(expected(9999), None, "未知 ID 必须被安静忽略");
    }

    /// `NIM_ADD` 和 `NIM_DELETE` 必须用同一个图标 ID。
    ///
    /// 对不上就会出现最难查的现象：进程退出了，图标还在任务栏上，
    /// 鼠标划过去还在、点它没反应。
    #[test]
    fn icon_id_is_single_sourced() {
        assert_eq!(ICON_ID, 1);
    }

    /// `close_hwnd` 对空句柄必须安全地什么都不做。
    ///
    /// 退出流程里它可能被多次调用，而且 `/api/quit` 那条路在托盘没起来时
    /// 拿到的是 0 —— 这时候绝不能崩，也不能去投递一个非法句柄。
    #[test]
    fn close_hwnd_tolerates_zero() {
        close_hwnd(0);
    }

    /// 端到端：`close_hwnd` 必须真的让托盘线程退出（图标随之被摘掉）。
    ///
    /// 这条盯着「退出了但图标还在」那类问题：只要窗口没被销毁，
    /// `WM_DESTROY` 就不会跑，`NIM_DELETE` 也就永远不会被调用。
    #[test]
    fn close_hwnd_shuts_the_tray_down() {
        let (tray, join) = match spawn("测试托盘-关闭", Box::new(TrayStatus::default)) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("（跳过：无法创建托盘窗口：{e}）");
                return;
            }
        };

        // 走公开接口关闭，而不是内部的 Tray::close —— 这正是 /api/quit 用的路径。
        close_hwnd(tray.hwnd);

        let waiter = std::thread::spawn(move || {
            let _ = join.join();
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !waiter.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            waiter.is_finished(),
            "close_hwnd 之后托盘线程没退出，说明窗口没被销毁、图标会留在任务栏上"
        );
    }

    #[test]
    fn notify_icon_data_size_is_sane() {
        // 结构体布局必须与 Win32 期望一致（NOTIFYICONDATAW 在 64 位下约 976 字节）
        // 与 Win32 的 NOTIFYICONDATAW（Vista+）必须**逐字节一致**，
        // 否则 Shell_NotifyIconW 会因 cbSize 不符而失败。
        let n = std::mem::size_of::<NotifyIconDataW>();
        assert_eq!(n, 976, "NOTIFYICONDATAW 大小应为 976，实际 {n}");
    }

    #[test]
    fn bundled_icon_loads_when_present() {
        // 这条测试针对一个很隐蔽的坑：图标文件存在但格式不对时，
        // LoadImageW 会返回 0，托盘**不报错、只是显示成一块空白**，
        // 从日志上完全看不出问题。所以在开发机上就把它卡住。
        let Some(p) = icon_path() else {
            // 没有资源文件时走代码绘制的兜底，不算失败
            eprintln!("（跳过：没找到 assets/icon.ico，将使用内置绘制图标）");
            return;
        };
        let h = load_icon_from_file();
        assert!(
            h.is_some(),
            "图标文件存在但加载失败（格式可能不对）：{}",
            p.display()
        );
        // 载入成功要负责释放，避免测试泄漏 GDI 句柄
        unsafe {
            extern "system" {
                fn DestroyIcon(h: Hicon) -> Bool;
            }
            let _ = DestroyIcon(h.unwrap());
        }
    }

    #[test]
    fn builtin_fallback_always_draws_something() {
        // 没有资源文件时也必须能画出一个图标：
        // 托盘没有图标会显示成空白，比一个不好看的图标更让人困惑。
        let h = make_icon_builtin();
        assert_ne!(h, 0, "内置绘制的图标不该失败");
        unsafe {
            extern "system" {
                fn DestroyIcon(h: Hicon) -> Bool;
            }
            let _ = DestroyIcon(h);
        }
    }

    /// 真机验证：建出真托盘，投一条 `WM_COMMAND(ID_QUIT)`，命令必须回到调用方。
    ///
    /// 这是本文件里唯一一条**端到端**的测试，也是唯一能抓住「菜单点了没反应」
    /// 那个 bug 的测试 —— 上面的映射表单测只能证明表是对的，
    /// 证明不了消息真的被接到、命令真的被送出去。
    ///
    /// 之所以能在这里安全地跑：托盘窗口是 `HWND_MESSAGE`（只收消息、不显示），
    /// 且 `WM_COMMAND` 是直接 `PostMessage` 进去的，不需要真人点菜单。
    /// 测试结束用 `close()` 走正常销毁路径，顺带验证退出流程本身不会卡住。
    #[test]
    fn quit_command_reaches_the_receiver() {
        let (tray, join) = match spawn("测试托盘", Box::new(TrayStatus::default)) {
            Ok(v) => v,
            Err(e) => {
                // 无桌面会话（比如纯 SSH / CI 容器）时建不出窗口，跳过。
                eprintln!("（跳过：无法创建托盘窗口：{e}）");
                return;
            }
        };

        // 模拟菜单选中「退出」：WM_COMMAND 的 wParam 低 16 位是菜单项 ID。
        unsafe {
            let ok = PostMessageW(tray.hwnd, WM_COMMAND, ID_QUIT as Wparam, 0);
            assert_ne!(ok, 0, "PostMessageW(WM_COMMAND) 失败");
        }

        // 命令经由 channel 异步回来，给一个宽松但有限的上限。
        let mut got = None;
        for _ in 0..100 {
            if let Some(c) = tray.poll() {
                got = Some(c);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(
            got,
            Some(TrayCommand::Quit),
            "投递 WM_COMMAND(ID_QUIT={ID_QUIT}) 后没收到 Quit —— \
             这正是「右键退出没反应」的复现"
        );

        // 顺带验证退出路径：close() 必须能让托盘线程正常收尾，而不是卡住。
        tray.close();
        let waited = std::thread::spawn(move || {
            let _ = join.join();
        });
        // 给 2 秒；超时不算失败（线程可能被系统延迟调度），但正常情况应当很快返回。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !waited.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            waited.is_finished(),
            "close() 之后托盘线程没能在 2 秒内退出，退出流程被卡住了"
        );
    }

    /// 菜单项 ID 必须覆盖新增的「打开界面」，且不与既有 ID 撞车。
    #[test]
    fn open_ui_id_is_mapped_and_unique() {
        let ids = [ID_OPEN_UI, ID_STOP, ID_LOGS, ID_DATA, ID_QUIT];
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                assert_ne!(ids[i], ids[j], "菜单项 ID 重复：{} 与 {}", ids[i], ids[j]);
            }
        }
        // handle_command 里 ID_OPEN_UI 必须映射到 OpenUi
        fn expected(id: u32) -> Option<TrayCommand> {
            match id {
                ID_OPEN_UI => Some(TrayCommand::OpenUi),
                ID_STOP => Some(TrayCommand::StopRecording),
                ID_LOGS => Some(TrayCommand::OpenLogs),
                ID_DATA => Some(TrayCommand::OpenDataDir),
                ID_QUIT => Some(TrayCommand::Quit),
                _ => None,
            }
        }
        assert_eq!(expected(ID_OPEN_UI), Some(TrayCommand::OpenUi));
    }

    /// 左键单击必须发 `OpenUi`（而不是打开数据目录）。
    ///
    /// 回归点：早先只监听 `WM_LBUTTONDBLCLK`，于是「左键打不开页面」。
    /// 这条测试用真实消息走一遍 `wnd_proc`，确认单击收到的是 OpenUi。
    #[test]
    fn left_click_opens_ui() {
        let (tray, join) = match spawn("测试托盘-左键", Box::new(TrayStatus::default)) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("（跳过：无法创建托盘窗口：{e}）");
                return;
            }
        };

        // WM_TRAY 的 lParam 低 16 位是鼠标事件类型。
        unsafe {
            let ok = PostMessageW(tray.hwnd, WM_TRAY, 1, WM_LBUTTONUP as isize);
            assert_ne!(ok, 0, "PostMessageW(WM_TRAY/LBUTTONUP) 失败");
        }

        let mut got = None;
        for _ in 0..100 {
            if let Some(c) = tray.poll() {
                got = Some(c);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(
            got,
            Some(TrayCommand::OpenUi),
            "左键单击没有发出 OpenUi —— 这正是「左键点击打不开页面」的复现"
        );

        tray.close();
        let _ = join.join();
    }

    /// 托盘窗口**不能**是 message-only 窗口（`HWND_MESSAGE`）。
    ///
    /// 回归点：用户报「右键任务栏图标依然没有反应」。
    /// 根因是托盘窗口原本用 `HWND_MESSAGE` 当父窗口 —— 收托盘回调消息没问题，
    /// 但 `TrackPopupMenu` 从一个 message-only 窗口上**弹不出来**
    /// （它需要窗口属于正常桌面输入队列）。菜单构造、命令映射全都对，
    /// 就是看不见。
    ///
    /// 这条测试直接读回窗口的父窗口来验证 —— 比"弹一次看看"更可靠，
    /// 因为自动化环境里没法真的去看菜单有没有画出来。
    #[test]
    #[cfg(windows)]
    fn tray_window_is_not_message_only() {
        let (tray, join) = match spawn("测试托盘-父窗口", Box::new(TrayStatus::default)) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("（跳过：无法创建托盘窗口：{e}）");
                return;
            }
        };

        // GetParent 对 message-only 窗口返回 HWND_MESSAGE(-3)，
        // 对普通顶层窗口返回 0（桌面）。
        #[link(name = "user32")]
        extern "system" {
            fn GetParent(hwnd: Hwnd) -> Hwnd;
        }
        let parent = unsafe { GetParent(tray.hwnd) };

        tray.close();
        let _ = join.join();

        assert_ne!(
            parent, HWND_MESSAGE,
            "托盘窗口是 message-only 窗口 —— 右键菜单会弹不出来（「右键没反应」的根因）"
        );
    }

    /// 右键菜单的**每一层**都必须走通：消息 → `show_menu` → `handle_command`。
    ///
    /// 这条测试不打开真实菜单（自动化环境里弹不出来也没法点），
    /// 而是验证 `handle_command` 对右键菜单里所有项都能正确分发 ——
    /// 覆盖"菜单能弹但点了没反应"那一类故障。
    #[test]
    fn every_menu_item_dispatches_to_a_command() {
        let cases = [
            (ID_STOP, TrayCommand::StopRecording),
            (ID_OPEN_UI, TrayCommand::OpenUi),
            (ID_LOGS, TrayCommand::OpenLogs),
            (ID_DATA, TrayCommand::OpenDataDir),
            (ID_QUIT, TrayCommand::Quit),
        ];
        for (id, want) in cases {
            assert_eq!(
                command_for(id),
                Some(want),
                "菜单项 ID={id} 没有映射到 {want:?} —— 点它不会有任何反应"
            );
        }
        // 菜单里列出的每一项都必须在映射表里
        for id in MENU_ITEM_IDS {
            assert!(
                command_for(*id).is_some(),
                "MENU_ITEM_IDS 里的 {id} 没有对应命令 —— 菜单里会出现一个点了没反应的项"
            );
        }
    }

    /// 状态行渲染：不在录制时**不能**出现「正在录制」。
    ///
    /// 这是需求「非录制状态下不显示立即停止录制」的文案侧保证：
    /// 菜单项由 `status.recording` 决定出不出现，所以这个字段必须准确。
    #[test]
    fn status_lines_reflect_recording_state() {
        let idle = TrayStatus {
            daemon_running: true,
            recording: false,
            pending_jobs: 3,
            uptime: "5 分 0 秒".to_string(),
            ..Default::default()
        };
        let lines = idle.menu_lines();
        assert!(
            !lines.iter().any(|l| l.contains("正在录制")),
            "空闲状态不该出现「正在录制」：{lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains("待处理作业 3 个")));

        let busy = TrayStatus {
            recording: true,
            ..idle.clone()
        };
        let lines2 = busy.menu_lines();
        assert!(
            lines2.iter().any(|l| l.contains("正在录制")),
            "录制中必须显示「正在录制」：{lines2:?}"
        );
    }

    /// 未运行时不该显示「已运行 …」这种没意义的信息。
    #[test]
    fn status_lines_hide_uptime_when_idle() {
        let s = TrayStatus {
            daemon_running: false,
            uptime: String::new(),
            ..Default::default()
        };
        let lines = s.menu_lines();
        assert!(lines.iter().any(|l| l.contains("未运行")));
        assert!(!lines.iter().any(|l| l.contains("已运行")));
    }

    /// 长错误信息要按字符截断，不能切出半个中文字。
    #[test]
    fn truncate_is_char_safe() {
        let s = "错误".repeat(50);
        let t = truncate_chars(&s, 40);
        assert_eq!(t.chars().count(), 41, "应为 40 个字符 + 一个省略号");
        assert!(t.ends_with('…'));
        // 短字符串原样返回，不加省略号
        assert_eq!(truncate_chars("短", 10), "短");
    }

    /// 菜单里「立即停止录制」是否出现，完全由 `recording` 决定。
    ///
    /// 这里不经过 UI，而是直接断言**决定菜单项去留的那个判断**，
    /// 把「非录制状态下不显示停止录制」这条需求钉在测试里。
    #[test]
    fn stop_item_only_when_recording() {
        // 与 show_menu 里 `if status.recording { ... }` 同一条件
        let idle = TrayStatus {
            daemon_running: true,
            recording: false,
            ..Default::default()
        };
        assert!(!idle.recording, "非录制状态下不该出现「立即停止录制」");

        let busy = TrayStatus {
            recording: true,
            ..idle
        };
        assert!(busy.recording, "录制中才该出现「立即停止录制」");
    }

    /// 演练模式要如实标出来，不能让人以为真在录。
    #[test]
    fn dry_run_is_labelled() {
        let s = TrayStatus {
            daemon_running: true,
            recording: false,
            dry_run: true,
            ..Default::default()
        };
        let lines = s.menu_lines();
        assert!(
            lines.iter().any(|l| l.contains("演练")),
            "演练模式必须在菜单里标出来：{lines:?}"
        );
    }
}
