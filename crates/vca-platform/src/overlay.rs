//! 桌面悬浮窗（Windows 专用）。
//!
//! 需求：「每次上课结束 5 分钟后在最前方桌面右上角显示一个小悬浮窗，
//! 悬浮窗内写 `Class Overrr :)`，上课前两分钟时关闭悬浮窗。」
//!
//! 本模块只负责**显示/隐藏一个窗口**（哑视图）；
//! 「何时显示、何时关闭」由 [`vca_core::schedule`] 计算，避免把时间逻辑混进 UI。
//!
//! 窗口特性：
//! - 无边框、无标题栏、无任务栏按钮（`WS_POPUP` + `WS_EX_TOOLWINDOW`）；
//! - 始终置顶（`WS_EX_TOPMOST`）；
//! - **不抢占焦点**（`WS_EX_NOACTIVATE` + `SW_SHOWNOACTIVATE`），不会打断教师操作；
//! - 圆角、纯色背景，绘制固定文本。
//!
//! 线程模型：窗口在**独立线程**里创建并跑消息循环，
//! 通过 `PostMessage`（`WM_APP+1` / `WM_APP+2`）从任意线程控制显示与隐藏，全程事件驱动、零轮询。

use std::sync::mpsc;

// ---------------------------------------------------------------------------
// Win32 基础类型
// ---------------------------------------------------------------------------
type Hwnd = isize;
type Hinstance = isize;
type Hicon = isize;
type Hcursor = isize;
type Hbrush = isize;
type Hfont = isize;
type Hdc = isize;
type Hmenu = isize;

type Lpcwstr = *const u16;
type Uint = u32;
type Wparam = usize;
type Lparam = isize;
type Lresult = isize;
type Bool = i32;

const WS_POPUP: u32 = 0x8000_0000;
const WS_EX_TOPMOST: u32 = 0x0000_0008;
const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
const WS_EX_TRANSPARENT: u32 = 0x0000_0020;
const WS_EX_LAYERED: u32 = 0x0008_0000;

const LWA_ALPHA: u32 = 0x0000_0002;

const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_CAPTURECHANGED: u32 = 0x0215;

const SW_HIDE: i32 = 0;
const SW_SHOWNOACTIVATE: i32 = 4;

const WM_DESTROY: u32 = 0x0002;
const WM_CLOSE: u32 = 0x0010;
const WM_PAINT: u32 = 0x000F;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_APP_SHOW: u32 = 0x8000 + 1;
const WM_APP_HIDE: u32 = 0x8000 + 2;

const HWND_TOPMOST: Hwnd = -1;
#[allow(dead_code)]
const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOACTIVATE: u32 = 0x0010;
#[allow(dead_code)]
const SWP_SHOWWINDOW: u32 = 0x0040;

const SM_CXSCREEN: i32 = 0;
const SM_CYSCREEN: i32 = 1;

const DT_CENTER: u32 = 0x0000_0001;
const DT_VCENTER: u32 = 0x0000_0004;
const DT_SINGLELINE: u32 = 0x0000_0020;

const TRANSPARENT_BK: i32 = 1;
const IDC_ARROW: Lpcwstr = 32512usize as Lpcwstr;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
struct Msg {
    hwnd: Hwnd,
    message: Uint,
    w_param: Wparam,
    l_param: Lparam,
    time: u32,
    pt: Point,
}

#[repr(C)]
struct PaintStruct {
    hdc: Hdc,
    f_erase: Bool,
    rc_paint: Rect,
    f_restore: Bool,
    f_inc_update: Bool,
    rgb_reserved: [u8; 32],
}

#[repr(C)]
struct WndClassW {
    style: Uint,
    lpfn_wnd_proc: Option<unsafe extern "system" fn(Hwnd, Uint, Wparam, Lparam) -> Lresult>,
    cb_cls_extra: i32,
    cb_wnd_extra: i32,
    h_instance: Hinstance,
    h_icon: Hicon,
    h_cursor: Hcursor,
    hbr_background: Hbrush,
    lpsz_menu_name: Lpcwstr,
    lpsz_class_name: Lpcwstr,
}

#[link(name = "user32")]
extern "system" {
    fn RegisterClassW(lp_wnd_class: *const WndClassW) -> u16;
    fn CreateWindowExW(
        dw_ex_style: Uint,
        lp_class_name: Lpcwstr,
        lp_window_name: Lpcwstr,
        dw_style: Uint,
        x: i32,
        y: i32,
        n_width: i32,
        n_height: i32,
        h_wnd_parent: Hwnd,
        h_menu: Hmenu,
        h_instance: Hinstance,
        lp_param: *mut core::ffi::c_void,
    ) -> Hwnd;
    fn DefWindowProcW(hwnd: Hwnd, msg: Uint, w_param: Wparam, l_param: Lparam) -> Lresult;
    fn ShowWindow(hwnd: Hwnd, n_cmd_show: i32) -> Bool;
    fn UpdateWindow(hwnd: Hwnd) -> Bool;
    fn DestroyWindow(hwnd: Hwnd) -> Bool;
    fn PostQuitMessage(n_exit_code: i32);
    fn GetMessageW(
        lp_msg: *mut Msg,
        hwnd: Hwnd,
        w_msg_filter_min: Uint,
        w_msg_filter_max: Uint,
    ) -> Bool;
    fn TranslateMessage(lp_msg: *const Msg) -> Bool;
    fn DispatchMessageW(lp_msg: *const Msg) -> Lresult;
    fn PostMessageW(hwnd: Hwnd, msg: Uint, w_param: Wparam, l_param: Lparam) -> Bool;
    fn GetSystemMetrics(n_index: i32) -> i32;
    fn SetWindowPos(
        hwnd: Hwnd,
        h_wnd_insert_after: Hwnd,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        u_flags: Uint,
    ) -> Bool;
    fn BeginPaint(hwnd: Hwnd, lp_paint: *mut PaintStruct) -> Hdc;
    fn EndPaint(hwnd: Hwnd, lp_paint: *const PaintStruct) -> Bool;
    fn FillRect(hdc: Hdc, lprc: *const Rect, hbr: Hbrush) -> i32;
    fn SetTextColor(hdc: Hdc, color: u32) -> u32;
    fn SetBkMode(hdc: Hdc, mode: i32) -> i32;
    fn DrawTextW(
        hdc: Hdc,
        lpch_text: Lpcwstr,
        n_count: i32,
        lp_rect: *mut Rect,
        u_format: Uint,
    ) -> i32;
    fn GetModuleHandleW(lp_module_name: Lpcwstr) -> Hinstance;
    fn LoadCursorW(h_instance: Hinstance, lp_cursor_name: Lpcwstr) -> Hcursor;
    fn IsWindow(hwnd: Hwnd) -> Bool;
    fn SetLayeredWindowAttributes(hwnd: Hwnd, key: u32, alpha: u8, flags: u32) -> Bool;
    fn SetCapture(hwnd: Hwnd) -> Hwnd;
    fn ReleaseCapture() -> Bool;
    fn GetCursorPos(p: *mut Point) -> Bool;
    fn GetWindowRect(hwnd: Hwnd, r: *mut Rect) -> Bool;
    fn GetDC(hwnd: Hwnd) -> Hdc;
    fn ReleaseDC(hwnd: Hwnd, hdc: Hdc) -> i32;
    fn SetWindowCompositionAttribute(hwnd: Hwnd, data: *mut WindowCompositionAttribData) -> Bool;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateSolidBrush(color: u32) -> Hbrush;
    fn DeleteObject(ho: isize) -> Bool;
    fn CreateFontW(
        c_height: i32,
        c_width: i32,
        c_escapement: i32,
        c_orientation: i32,
        c_weight: i32,
        b_italic: Uint,
        b_underline: Uint,
        b_strike_out: Uint,
        i_char_set: Uint,
        i_out_precision: Uint,
        i_clip_precision: Uint,
        i_quality: Uint,
        i_pitch_and_family: Uint,
        psz_face_name: Lpcwstr,
    ) -> Hfont;
    fn SelectObject(hdc: Hdc, h: isize) -> isize;
}

// 说明：本机 Rust 自带的 MinGW 导入库不含 dwmapi，故不链接 DWM。
// 液态玻璃改用 user32 的 SetWindowCompositionAttribute（Win10 1803+ 通用），
// 主机色改用注册表读取（advapi32），二者均可在自带库中找到。

#[link(name = "advapi32")]
extern "system" {
    fn RegGetValueW(
        hkey: isize,
        sub_key: *const u16,
        value: *const u16,
        flags: u32,
        out_type: *mut u32,
        out_data: *mut core::ffi::c_void,
        out_size: *mut u32,
    ) -> i32;
}

/// 窗口合成属性（未公开但长期稳定，Win10 1803+ 的亚克力效果入口）。
#[repr(C)]
struct AccentPolicy {
    accent_state: u32,
    accent_flags: u32,
    gradient_color: u32,
    animation_id: u32,
}

#[repr(C)]
struct WindowCompositionAttribData {
    attrib: u32,
    pv_data: *mut core::ffi::c_void,
    cb_data: usize,
}

/// `WCA_ACCENT_POLICY`
const WCA_ACCENT_POLICY: u32 = 19;
/// 亚克力模糊（Win10 1803+）；不可用时退化为普通模糊。
#[allow(dead_code)]
const ACCENT_ENABLE_ACRYLICBLURBEHIND: u32 = 4;
/// 普通模糊（Win10 早期）。
#[allow(dead_code)]
const ACCENT_ENABLE_BLURBEHIND: u32 = 3;
/// 完全透明（用于关闭效果）。
const ACCENT_DISABLED: u32 = 0;

const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as isize;
const RRF_RT_REG_DWORD: u32 = 0x0000_0010;

// ---------------------------------------------------------------------------
// 系统主机色（Windows 强调色）
// ---------------------------------------------------------------------------

/// 读取 Windows「主机色 / 强调色」，返回 Win32 `COLORREF`（0x00BBGGRR）。
///
/// 数据来源：注册表 `HKCU\SOFTWARE\Microsoft\Windows\DWM\AccentColor`，
/// 其为 `0xAABBGGRR`，低 24 位与 Win32 `COLORREF`（0x00BBGGRR）字节序一致。
///
/// 取不到时返回 `None`，调用方回退到配置中的固定颜色。
pub fn read_system_accent_color() -> Option<u32> {
    // SAFETY: 所有指针均指向本函数内的栈变量；字符串缓冲区以 NUL 结尾。
    unsafe {
        let sub = wide("SOFTWARE\\Microsoft\\Windows\\DWM");
        let val = wide("AccentColor");
        let mut data: u32 = 0;
        let mut size: u32 = core::mem::size_of::<u32>() as u32;
        let mut ty: u32 = 0;
        let rc = RegGetValueW(
            HKEY_CURRENT_USER,
            sub.as_ptr(),
            val.as_ptr(),
            RRF_RT_REG_DWORD,
            &mut ty,
            &mut data as *mut u32 as *mut core::ffi::c_void,
            &mut size,
        );
        if rc == 0 {
            // 注册表中为 0xAABBGGRR，低 24 位与 COLORREF 一致
            let c = data & 0x00FF_FFFF;
            if c != 0 {
                return Some(c);
            }
        }

        None
    }
}

/// 按背景亮度自动选择可读的文字色（返回 COLORREF）。
pub fn readable_text_color(bg: u32) -> u32 {
    let r = (bg & 0xFF) as f32;
    let g = ((bg >> 8) & 0xFF) as f32;
    let b = ((bg >> 16) & 0xFF) as f32;
    // 相对亮度（sRGB 近似）
    let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    if luma > 140.0 {
        rgb(0x1A, 0x1A, 0x1A) // 浅色玻璃 -> 深色字
    } else {
        rgb(0xFF, 0xFF, 0xFF) // 深色玻璃 -> 白字
    }
}

/// 应用「液态玻璃」效果：亚克力模糊 + Win11 圆角与背景材质。
///
/// `tint` 为叠加在玻璃上的色调（COLORREF），`alpha` 为不透明度（0-255）。
/// 失败时静默跳过（旧系统不支持亚克力时窗口仍可正常显示）。
pub fn apply_liquid_glass(hwnd: Hwnd, _tint: u32, alpha: u8) {
    if hwnd == 0 {
        return;
    }
    // 使用分层窗口的整体透明度实现「半透明玻璃」。
    //
    // 说明：这里刻意**不用** SetWindowCompositionAttribute 的亚克力效果
    // 它会让 DWM 接管窗口背景，导致本进程用 GDI 绘制的文字与圆角不可见。
    // 改用 LWA_ALPHA 后，背景与文字都由我们自己绘制，可控且稳定。
    // SAFETY: hwnd 由本模块创建；SetLayeredWindowAttributes 只设置属性。
    unsafe {
        let a = if alpha == 0 { 200 } else { alpha.max(40) };
        let _ = SetLayeredWindowAttributes(hwnd, 0, a, LWA_ALPHA);
    }
}

/// 关闭玻璃效果（恢复为不透明窗口）。
pub fn disable_liquid_glass(hwnd: Hwnd) {
    if hwnd == 0 {
        return;
    }
    // SAFETY: 同 apply_liquid_glass。
    unsafe {
        let mut policy = AccentPolicy {
            accent_state: ACCENT_DISABLED,
            accent_flags: 0,
            gradient_color: 0,
            animation_id: 0,
        };
        let mut data = WindowCompositionAttribData {
            attrib: WCA_ACCENT_POLICY,
            pv_data: &mut policy as *mut AccentPolicy as *mut core::ffi::c_void,
            cb_data: core::mem::size_of::<AccentPolicy>(),
        };
        let _ = SetWindowCompositionAttribute(hwnd, &mut data);
    }
}

/// 圆角矩形的有符号距离场（负值表示在内部）。
fn rounded_sdf(px: f32, py: f32, w: f32, h: f32, r: f32) -> f32 {
    let hx = w * 0.5;
    let hy = h * 0.5;
    let qx = (px - hx).abs() - (hx - r);
    let qy = (py - hy).abs() - (hy - r);
    let ax = qx.max(0.0);
    let ay = qy.max(0.0);
    (ax * ax + ay * ay).sqrt() + qx.max(qy).min(0.0) - r
}

/// 某像素被圆角矩形覆盖的比例（0..1）。用 4x4 超采样实现抗锯齿。
fn corner_coverage(x: i32, y: i32, w: i32, h: i32, r: f32) -> f32 {
    const N: i32 = 4;
    let mut hit = 0;
    for sy in 0..N {
        for sx in 0..N {
            let px = x as f32 + (sx as f32 + 0.5) / N as f32;
            let py = y as f32 + (sy as f32 + 0.5) / N as f32;
            if rounded_sdf(px, py, w as f32, h as f32, r) <= 0.0 {
                hit += 1;
            }
        }
    }
    hit as f32 / (N * N) as f32
}

/// 按比例把两个 COLORREF 混合（t=0 取 a，t=1 取 b）。
fn blend(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let (ar, ag, ab) = (a & 0xFF, (a >> 8) & 0xFF, (a >> 16) & 0xFF);
    let (br, bg, bb) = (b & 0xFF, (b >> 8) & 0xFF, (b >> 16) & 0xFF);
    let r = ar as f32 + (br as f32 - ar as f32) * t;
    let g = ag as f32 + (bg as f32 - ag as f32) * t;
    let bl = ab as f32 + (bb as f32 - ab as f32) * t;
    rgb(r as u8, g as u8, bl as u8)
}

/// 提亮一个颜色（用于玻璃上沿的高光）。
fn lighten(c: u32, amount: f32) -> u32 {
    blend(c, rgb(0xFF, 0xFF, 0xFF), amount)
}

/// 压暗一个颜色（用于玻璃下沿）。
fn darken(c: u32, amount: f32) -> u32 {
    blend(c, rgb(0x00, 0x00, 0x00), amount)
}

// ---------------------------------------------------------------------------
// 样式存储：进程级静态，避免依赖 GWLP_USERDATA（少一个失败点）
// ---------------------------------------------------------------------------
static STYLE: std::sync::Mutex<Option<OverlayStyle>> = std::sync::Mutex::new(None);

/// 拖动状态：(是否正在拖动, 按下时的光标 X, 按下时的光标 Y, 按下时的窗口 X, 窗口 Y)
static DRAG: std::sync::Mutex<(bool, i32, i32, i32, i32)> =
    std::sync::Mutex::new((false, 0, 0, 0, 0));

fn set_style(st: OverlayStyle) {
    if let Ok(mut g) = STYLE.lock() {
        *g = Some(st);
    }
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

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct BlendFunction {
    blend_op: u8,
    blend_flags: u8,
    source_constant_alpha: u8,
    alpha_format: u8,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Size {
    cx: i32,
    cy: i32,
}

const DIB_RGB_COLORS: u32 = 0;
const BI_RGB: u32 = 0;
const AC_SRC_OVER: u8 = 0;
const AC_SRC_ALPHA: u8 = 1;
const ULW_ALPHA: u32 = 0x0000_0002;

#[link(name = "gdi32")]
extern "system" {
    fn CreateCompatibleDC(hdc: Hdc) -> Hdc;
    fn DeleteDC(hdc: Hdc) -> Bool;
    fn CreateDIBSection(
        hdc: Hdc,
        pbmi: *const BitmapInfo,
        usage: u32,
        bits: *mut *mut core::ffi::c_void,
        section: isize,
        offset: u32,
    ) -> isize;
    fn UpdateLayeredWindow(
        hwnd: Hwnd,
        hdc_dst: Hdc,
        ppt_dst: *const Point,
        psize: *const Size,
        hdc_src: Hdc,
        ppt_src: *const Point,
        cr_key: u32,
        pblend: *const BlendFunction,
        dw_flags: u32,
    ) -> Bool;
}

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn rgb(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

const CLASS_NAME: &str = "VcaOverlayWnd";

/// 窗口样式参数。
#[derive(Debug, Clone)]
pub struct OverlayStyle {
    /// 显示的文本。
    pub text: String,
    /// 窗口宽度（像素）。
    pub width: i32,
    /// 窗口高度（像素）。
    pub height: i32,
    /// 距屏幕上边缘的距离（像素）。
    pub margin_top: i32,
    /// 距屏幕右边缘的距离（像素）。
    pub margin_right: i32,
    /// 背景色（Win32 COLORREF，0x00BBGGRR）。
    pub bg_rgb: u32,
    /// 文字色（Win32 COLORREF）。
    pub text_rgb: u32,
    /// 字号（逻辑像素，负值表示字符高度）。
    pub font_size: i32,
    /// 圆角半径（像素）。
    pub corner_radius: i32,
    /// 是否鼠标穿透（点击不拦截下方窗口）。
    pub click_through: bool,
    /// 字体名（默认 Consolas）。
    pub font_face: String,
    /// 是否启用「液态玻璃」背景（亚克力模糊）。
    pub liquid_glass: bool,
    /// 玻璃层不透明度（0-255，越大越不透明）。
    pub glass_alpha: u8,
    /// 是否使用 Windows 系统「主机色」作为玻璃色调。
    pub use_system_accent: bool,
}

impl Default for OverlayStyle {
    fn default() -> Self {
        Self {
            text: "Agent Overrr :)".to_string(),
            width: 240,
            height: 72,
            margin_top: 16,
            margin_right: 16,
            bg_rgb: rgb(0x1F, 0x6F, 0xEB),
            text_rgb: rgb(0xFF, 0xFF, 0xFF),
            font_size: 18,
            corner_radius: 14,
            click_through: false,
            font_face: "Consolas".to_string(),
            liquid_glass: true,
            glass_alpha: 160,
            use_system_accent: true,
        }
    }
}

/// 悬浮窗句柄（可跨线程克隆使用）。
#[derive(Debug, Clone, Copy)]
pub struct Overlay {
    hwnd: Hwnd,
}

impl Overlay {
    /// 显示窗口（不抢焦点，保持置顶）。
    pub fn show(&self) {
        if !self.alive() {
            return;
        }
        // SAFETY: hwnd 由本模块创建并校验存活；PostMessage 只投递消息，不解引用指针。
        unsafe {
            PostMessageW(self.hwnd, WM_APP_SHOW, 0, 0);
        }
    }

    /// 隐藏窗口。
    pub fn hide(&self) {
        if !self.alive() {
            return;
        }
        // SAFETY: 同上。
        unsafe {
            PostMessageW(self.hwnd, WM_APP_HIDE, 0, 0);
        }
    }

    /// 请求销毁窗口并结束消息循环。
    pub fn close(&self) {
        if !self.alive() {
            return;
        }
        // SAFETY: 同上。
        unsafe {
            PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
        }
    }

    /// 窗口是否仍然有效。
    pub fn alive(&self) -> bool {
        if self.hwnd == 0 {
            return false;
        }
        // SAFETY: IsWindow 只做句柄有效性判断。
        unsafe { IsWindow(self.hwnd) != 0 }
    }

    /// 原始句柄值（用于日志）。
    pub fn raw(&self) -> isize {
        self.hwnd
    }
}

/// 在专用线程中创建悬浮窗。
///
/// 返回 `(句柄, 线程 JoinHandle)`；窗口创建失败时返回 `Err`。
pub fn spawn(style: OverlayStyle) -> std::io::Result<(Overlay, std::thread::JoinHandle<()>)> {
    let (tx, rx) = mpsc::channel::<Result<Hwnd, String>>();

    let join = std::thread::Builder::new()
        .name("vca-overlay".to_string())
        .spawn(move || {
            // SAFETY: 以下调用均为标准 Win32 窗口创建流程；
            // 传入的字符串缓冲区在调用期间保持存活。
            unsafe {
                let h_instance = GetModuleHandleW(core::ptr::null());
                let class_name = wide(CLASS_NAME);
                let wc = WndClassW {
                    style: 0,
                    lpfn_wnd_proc: Some(window_proc),
                    cb_cls_extra: 0,
                    cb_wnd_extra: 0,
                    h_instance,
                    h_icon: 0,
                    h_cursor: LoadCursorW(0, IDC_ARROW),
                    hbr_background: 0,
                    lpsz_menu_name: core::ptr::null(),
                    lpsz_class_name: class_name.as_ptr(),
                };
                let atom = RegisterClassW(&wc);
                if atom == 0 {
                    let _ = tx.send(Err("RegisterClassW 失败".to_string()));
                    return;
                }

                set_style(style.clone());
                let title = wide(&style.text);

                let mut ex_style =
                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED;
                if style.click_through {
                    ex_style |= WS_EX_TRANSPARENT;
                }

                let (x, y) = top_right_position(&style);
                let hwnd = CreateWindowExW(
                    ex_style,
                    class_name.as_ptr(),
                    title.as_ptr(),
                    WS_POPUP,
                    x,
                    y,
                    style.width,
                    style.height,
                    0,
                    0,
                    h_instance,
                    core::ptr::null_mut(),
                );

                if hwnd == 0 {
                    set_style(OverlayStyle::default());
                    let _ = tx.send(Err("CreateWindowExW 失败".to_string()));
                    return;
                }

                let _ = tx.send(Ok(hwnd));

                // 消息循环
                let mut msg: Msg = core::mem::zeroed();
                while GetMessageW(&mut msg, 0, 0, 0) > 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        })?;

    match rx.recv() {
        Ok(Ok(hwnd)) => Ok((Overlay { hwnd }, join)),
        Ok(Err(e)) => Err(std::io::Error::other(e)),
        Err(_) => Err(std::io::Error::other("悬浮窗线程提前退出")),
    }
}

/// 解析实际使用的前景色与玻璃色调。
///
/// 开启 `use_system_accent` 时读取 Windows 主机色；取不到则回退到 `bg_rgb`。
/// 文字颜色在玻璃模式下自动按亮度选择，保证可读。
pub fn resolve_colors(style: &OverlayStyle) -> (u32, u32) {
    let tint = if style.use_system_accent {
        read_system_accent_color().unwrap_or(style.bg_rgb)
    } else {
        style.bg_rgb
    };
    let text = if style.liquid_glass {
        readable_text_color(tint)
    } else {
        style.text_rgb
    };
    (tint, text)
}

/// 渲染并提交分层窗口内容（逐像素 Alpha，圆角带抗锯齿）。
///
/// 流程：
/// 1. 直接把背景（渐变 + 抗锯齿圆角）写进 32 位 DIB 的原始像素；
/// 2. 文字先用 GDI 画到一张「已知底色」的临时 DIB，再按 (C-B)/(T-B) 反解出覆盖率，
///    在 Rust 里做 over 合成  这样文字边缘也能获得抗锯齿；
/// 3. `UpdateLayeredWindow` 一次性提交，由 DWM 按 Alpha 合成到桌面。
///
/// 相比 `SetWindowRgn`：区域是 1 位掩码，圆角必然是硬边；本方案为 8 位 Alpha，边缘平滑。
pub fn render_layered(hwnd: Hwnd) -> bool {
    let Some(st) = style_of(hwnd) else {
        return false;
    };
    let (w, h) = (st.width.max(1), st.height.max(1));
    let (tint, text_rgb) = resolve_colors(&st);
    let radius = st.corner_radius.max(0) as f32;
    let glass = if st.liquid_glass {
        st.glass_alpha.max(1) as f32 / 255.0
    } else {
        1.0
    };

    // SAFETY: 所有句柄均由下方创建并在末尾释放；像素缓冲区长度按 w*h*4 精确计算。
    unsafe {
        let screen = GetDC(0);
        let mem = CreateCompatibleDC(screen);

        let mut bmi = BitmapInfo::default();
        bmi.header.size = core::mem::size_of::<BitmapInfoHeader>() as u32;
        bmi.header.width = w;
        bmi.header.height = -h; // 负值 = 自上而下
        bmi.header.planes = 1;
        bmi.header.bit_count = 32;
        bmi.header.compression = BI_RGB;

        let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
        let dib = CreateDIBSection(mem, &bmi, DIB_RGB_COLORS, &mut bits, 0, 0);
        if dib == 0 || bits.is_null() {
            let _ = DeleteDC(mem);
            let _ = ReleaseDC(0, screen);
            return false;
        }
        let old_bmp = SelectObject(mem, dib);

        let px = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);

        // ---- 背景 ----
        let top = lighten(tint, 0.22);
        let bottom = darken(tint, 0.18);
        for y in 0..h {
            let t = y as f32 / h as f32;
            let c = blend(top, bottom, t);
            let (r, g, b) = (
                (c & 0xFF) as f32,
                ((c >> 8) & 0xFF) as f32,
                ((c >> 16) & 0xFF) as f32,
            );
            for x in 0..w {
                let cov = if radius > 0.0 {
                    corner_coverage(x, y, w, h, radius)
                } else {
                    1.0
                };
                if cov <= 0.0 {
                    continue; // 已由 DIB 初始化为全 0（透明）
                }
                let a = glass * cov;
                let off = ((y * w + x) * 4) as usize;
                // 预乘 Alpha：BGRA
                px[off] = (b * a) as u8;
                px[off + 1] = (g * a) as u8;
                px[off + 2] = (r * a) as u8;
                px[off + 3] = (a * 255.0) as u8;
            }
        }

        // 顶部高光（玻璃反光），同样预乘
        {
            let hi = lighten(tint, 0.45);
            let (hr, hg, hb) = (
                (hi & 0xFF) as f32,
                ((hi >> 8) & 0xFF) as f32,
                ((hi >> 16) & 0xFF) as f32,
            );
            let y = 0;
            for x in 0..w {
                let cov = if radius > 0.0 {
                    corner_coverage(x, y, w, h, radius)
                } else {
                    1.0
                };
                if cov <= 0.0 {
                    continue;
                }
                let a = glass * cov;
                let off = ((y * w + x) * 4) as usize;
                px[off] = (hb * a) as u8;
                px[off + 1] = (hg * a) as u8;
                px[off + 2] = (hr * a) as u8;
                px[off + 3] = (a * 255.0) as u8;
            }
        }

        // ---- 文字（GDI 画到临时 DIB，再反解覆盖率做合成）----
        let tbmi = BitmapInfo {
            header: bmi.header,
            colors: bmi.colors,
        };
        let mut tbits: *mut core::ffi::c_void = core::ptr::null_mut();
        let tdib = CreateDIBSection(mem, &tbmi, DIB_RGB_COLORS, &mut tbits, 0, 0);
        if tdib != 0 && !tbits.is_null() {
            let tmem = CreateCompatibleDC(screen);
            let old_t = SelectObject(tmem, tdib);

            // 底色纯黑、文字纯白 -> 覆盖率 = 亮度
            let black = Rect {
                left: 0,
                top: 0,
                right: w,
                bottom: h,
            };
            let bbrush = CreateSolidBrush(0);
            if bbrush != 0 {
                FillRect(tmem, &black, bbrush);
                DeleteObject(bbrush);
            }
            SetBkMode(tmem, TRANSPARENT_BK);
            SetTextColor(tmem, 0x00FF_FFFF);
            let face = wide(&st.font_face);
            let font = CreateFontW(
                -st.font_size,
                0,
                0,
                0,
                600,
                0,
                0,
                0,
                1,
                0,
                0,
                5,
                0,
                face.as_ptr(),
            );
            let old_font = if font != 0 {
                SelectObject(tmem, font)
            } else {
                0
            };
            let text: Vec<u16> = st.text.encode_utf16().chain(std::iter::once(0)).collect();
            let mut trc = black;
            DrawTextW(
                tmem,
                text.as_ptr(),
                -1,
                &mut trc,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
            if old_font != 0 {
                SelectObject(tmem, old_font);
            }
            if font != 0 {
                DeleteObject(font);
            }

            let tp = std::slice::from_raw_parts(tbits as *const u8, (w * h * 4) as usize);
            let (tr, tg, tb) = (
                (text_rgb & 0xFF) as f32,
                ((text_rgb >> 8) & 0xFF) as f32,
                ((text_rgb >> 16) & 0xFF) as f32,
            );
            for i in 0..(w * h) as usize {
                let off = i * 4;
                // 临时图是黑底白字，取 B/G/R 最大者作为覆盖率
                let cov = (tp[off].max(tp[off + 1]).max(tp[off + 2])) as f32 / 255.0;
                if cov <= 0.0 {
                    continue;
                }
                let a0 = px[off + 3] as f32 / 255.0;
                let na = cov + a0 * (1.0 - cov);
                // over 合成：文字不透明
                let nr = tr * cov + px[off + 2] as f32 * (1.0 - cov);
                let ng = tg * cov + px[off + 1] as f32 * (1.0 - cov);
                let nb = tb * cov + px[off] as f32 * (1.0 - cov);
                px[off] = nb.min(255.0) as u8;
                px[off + 1] = ng.min(255.0) as u8;
                px[off + 2] = nr.min(255.0) as u8;
                px[off + 3] = (na * 255.0).min(255.0) as u8;
            }

            SelectObject(tmem, old_t);
            let _ = DeleteObject(tdib);
            let _ = DeleteDC(tmem);
        }

        // ---- 提交 ----
        let mut wr: Rect = core::mem::zeroed();
        GetWindowRect(hwnd, &mut wr);
        let dst = Point {
            x: wr.left,
            y: wr.top,
        };
        let size = Size { cx: w, cy: h };
        let src = Point { x: 0, y: 0 };
        let blend = BlendFunction {
            blend_op: AC_SRC_OVER,
            blend_flags: 0,
            source_constant_alpha: 255,
            alpha_format: AC_SRC_ALPHA,
        };
        let ok = UpdateLayeredWindow(hwnd, screen, &dst, &size, mem, &src, 0, &blend, ULW_ALPHA);

        SelectObject(mem, old_bmp);
        let _ = DeleteObject(dib);
        let _ = DeleteDC(mem);
        let _ = ReleaseDC(0, screen);
        ok != 0
    }
}

/// 计算右上角坐标。
fn top_right_position(style: &OverlayStyle) -> (i32, i32) {
    // SAFETY: GetSystemMetrics 只读取系统度量值。
    let (sw, _sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let x = (sw - style.width - style.margin_right).max(0);
    let y = style.margin_top.max(0);
    (x, y)
}

unsafe extern "system" fn window_proc(
    hwnd: Hwnd,
    msg: Uint,
    w_param: Wparam,
    l_param: Lparam,
) -> Lresult {
    match msg {
        WM_APP_SHOW => {
            if let Some(s) = style_of(hwnd) {
                let (x, y) = top_right_position(&s);
                // 先移动到位（UpdateLayeredWindow 需要正确的窗口坐标）
                SetWindowPos(hwnd, HWND_TOPMOST, x, y, s.width, s.height, SWP_NOACTIVATE);
            }
            // 渲染并提交分层内容（含抗锯齿圆角）
            let _ = render_layered(hwnd);
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            UpdateWindow(hwnd);
            0
        }
        WM_APP_HIDE => {
            ShowWindow(hwnd, SW_HIDE);
            0
        }
        WM_PAINT => {
            // 分层窗口的内容由 render_layered 通过 UpdateLayeredWindow 提交；
            // 这里仍需响应 WM_PAINT 以满足系统对无效区域的校验。
            let mut ps: PaintStruct = core::mem::zeroed();
            let _ = BeginPaint(hwnd, &mut ps);
            render_layered(hwnd);
            let _ = EndPaint(hwnd, &ps);
            0
        }
        WM_ERASEBKGND => 1,
        WM_LBUTTONDOWN => {
            // 记录按下时的光标与窗口位置，进入拖动状态
            let mut pt = Point::default();
            let mut wr: Rect = core::mem::zeroed();
            GetCursorPos(&mut pt);
            GetWindowRect(hwnd, &mut wr);
            if let Ok(mut d) = DRAG.lock() {
                *d = (true, pt.x, pt.y, wr.left, wr.top);
            }
            SetCapture(hwnd);
            0
        }
        WM_MOUSEMOVE => {
            if let Ok(d) = DRAG.lock() {
                if d.0 {
                    let mut pt = Point::default();
                    GetCursorPos(&mut pt);
                    let nx = d.3 + (pt.x - d.1);
                    let ny = d.4 + (pt.y - d.2);
                    let (w, h) = with_size();
                    SetWindowPos(hwnd, HWND_TOPMOST, nx, ny, w, h, SWP_NOACTIVATE);
                    let _ = render_layered(hwnd);
                }
            }
            0
        }
        WM_LBUTTONUP => {
            if let Ok(mut d) = DRAG.lock() {
                d.0 = false;
            }
            ReleaseCapture();
            0
        }
        WM_CAPTURECHANGED => {
            if let Ok(mut d) = DRAG.lock() {
                d.0 = false;
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, w_param, l_param),
    }
}

/// 取出当前样式（进程内单例）。
/// 取当前窗口尺寸（拖动时需要原样传回 SetWindowPos）。
fn with_size() -> (i32, i32) {
    STYLE
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|s| (s.width, s.height)))
        .unwrap_or((240, 72))
}

fn style_of(_hwnd: Hwnd) -> Option<OverlayStyle> {
    STYLE.lock().ok().and_then(|g| g.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_packing_is_bgr() {
        // Win32 COLORREF 为 0x00BBGGRR
        assert_eq!(rgb(0xFF, 0x00, 0x00), 0x0000_00FF);
        assert_eq!(rgb(0x00, 0xFF, 0x00), 0x0000_FF00);
        assert_eq!(rgb(0x00, 0x00, 0xFF), 0x00FF_0000);
    }

    #[test]
    fn wide_is_null_terminated() {
        let w = wide("A");
        assert_eq!(w, vec![65u16, 0u16]);
    }

    #[test]
    fn default_style_matches_requirement() {
        let s = OverlayStyle::default();
        assert_eq!(s.text, "Agent Overrr :)");
        assert_eq!(s.font_face, "Consolas");
        assert!(s.liquid_glass);
    }
}
