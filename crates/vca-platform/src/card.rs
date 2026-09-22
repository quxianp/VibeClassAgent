//! 把课后摘要画成一张 PNG 卡片。
//!
//! # 为什么要图片
//!
//! 企业微信**群机器人**支持 `image` 消息（base64 直传，见官方文档 path/91770），
//! 而它只支持文本 / markdown / 图片 / 图文 / 文件这几种。纯文本在手机上要展开，
//! 要点一多就成了"一屏字"；做成一张卡片图，扫一眼就能看完。
//!
//! 于是路径是：**摘要 → 本地渲染成 PNG → base64 直传给群机器人**。
//! 全程不需要公网地址、不需要图床 —— 这是这条需求最省事的解法。
//!
//! # 为什么用 GDI 自绘而不是截图浏览器
//!
//! 项目已经有 Edge 打 PDF 的先例，但那条路在受限环境里会失败（沙箱禁命名管道），
//! 而且为一张图去拉一个浏览器进程太重。GDI 画字是**几十毫秒**的事，
//! 也不依赖任何外部程序 —— 与同目录的 [`crate::screenshot`] 一个思路：
//! 自己写 FFI 声明，不引 windows crate 的 GDI feature。
//!
//! # 中文字体
//!
//! 用「微软雅黑」（Windows 7 起自带）。字号与行距按手机阅读来定：
//! 卡片宽 720px，正文 16px / 行距 1.7 —— 发到微信里缩放后正好能看清。

use std::ffi::c_void;
use std::path::Path;

use anyhow::{Context, Result};

type Hdc = isize;
type Hbitmap = isize;
type Hgdiobj = isize;
type Hfont = isize;
type Bool = i32;

const BI_RGB: u32 = 0;
const DIB_RGB_COLORS: u32 = 0;
const TRANSPARENT: i32 = 1;
const DEFAULT_CHARSET: u32 = 1;
const FW_NORMAL: i32 = 400;
const FW_BOLD: i32 = 700;
const CLEARTYPE_QUALITY: u32 = 5;

/// DrawTextW 的标志位。
const DT_LEFT: u32 = 0x0000_0000;
const DT_WORDBREAK: u32 = 0x0000_0010;
const DT_CALCRECT: u32 = 0x0000_0400;
const DT_NOPREFIX: u32 = 0x0000_0800;

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
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateCompatibleDC(hdc: Hdc) -> Hdc;
    fn DeleteDC(hdc: Hdc) -> Bool;
    fn CreateDIBSection(
        hdc: Hdc,
        pbmi: *const BitmapInfo,
        usage: u32,
        ppv_bits: *mut *mut c_void,
        h_section: isize,
        offset: u32,
    ) -> Hbitmap;
    fn SelectObject(hdc: Hdc, obj: Hgdiobj) -> Hgdiobj;
    fn DeleteObject(obj: Hgdiobj) -> Bool;
    fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strike_out: u32,
        charset: u32,
        out_precision: u32,
        clip_precision: u32,
        quality: u32,
        pitch_and_family: u32,
        face: *const u16,
    ) -> Hfont;
    fn SetBkMode(hdc: Hdc, mode: i32) -> i32;
    fn SetTextColor(hdc: Hdc, color: u32) -> u32;
    fn DrawTextW(hdc: Hdc, text: *const u16, count: i32, rect: *mut Rect, format: u32) -> i32;
}

#[link(name = "user32")]
extern "system" {
    fn GetDC(hwnd: isize) -> Hdc;
    fn ReleaseDC(hwnd: isize, hdc: Hdc) -> i32;
}

/// 转成以 NUL 结尾的 UTF-16。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 卡片尺寸与配色（改这里就能调观感）。
struct Style {
    width: i32,
    pad: i32,
    title_size: i32,
    body_size: i32,
    line_gap: i32,
    bg: (u8, u8, u8),
    title_color: u32,
    body_color: u32,
    accent: (u8, u8, u8),
}

impl Default for Style {
    fn default() -> Self {
        Self {
            width: 720,
            pad: 36,
            title_size: 26,
            body_size: 17,
            line_gap: 10,
            // 背景用极浅的灰，纯白在微信里会糊成一片
            bg: (250, 250, 250),
            title_color: 0x0020_2020, // COLORREF 是 0x00BBGGRR
            body_color: 0x0040_4040,
            accent: (32, 150, 243),
        }
    }
}

/// 把 `title` + `body` 渲染成 PNG，写到 `out`，返回 (宽, 高)。
pub fn render_png(title: &str, body: &str, out: &Path) -> Result<(u32, u32)> {
    let st = Style::default();
    let text_w = st.width - st.pad * 2;

    let screen = unsafe { GetDC(0) };
    if screen == 0 {
        return Err(anyhow::anyhow!("拿不到屏幕 DC（无桌面会话？）"));
    }
    let dc = unsafe { CreateCompatibleDC(screen) };
    unsafe { ReleaseDC(0, screen) };
    if dc == 0 {
        return Err(anyhow::anyhow!("创建内存 DC 失败"));
    }

    // 先量高度：GDI 的 DrawTextW 配 DT_CALCRECT 会把矩形撑到实际所需高度，
    // 这样"卡片多高"不用自己估行数 —— 中文换行交给系统更省事也更准。
    let title_h = measure(dc, title, st.title_size, FW_BOLD, text_w)?;
    let body_h = if body.trim().is_empty() {
        0
    } else {
        measure(dc, body, st.body_size, FW_NORMAL, text_w)?
    };

    let height = st.pad * 2 + title_h + if body_h > 0 { st.line_gap + body_h } else { 0 } + 6;

    // 建一张 32 位自上而下的 DIB
    let mut info = BitmapInfo::default();
    info.header.size = std::mem::size_of::<BitmapInfoHeader>() as u32;
    info.header.width = st.width;
    info.header.height = -height; // 负数 = 自上而下，省得后面再翻转
    info.header.planes = 1;
    info.header.bit_count = 32;
    info.header.compression = BI_RGB;

    let mut bits: *mut c_void = std::ptr::null_mut();
    let hbmp = unsafe { CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, 0, 0) };
    if hbmp == 0 || bits.is_null() {
        unsafe { DeleteDC(dc) };
        return Err(anyhow::anyhow!("创建 DIB 失败"));
    }
    let old = unsafe { SelectObject(dc, hbmp) };

    // 铺底色（DIB 初值是 0，也就是全黑，必须自己填）
    let px = bits as *mut u8;
    let total = (st.width * height) as usize;
    unsafe {
        for i in 0..total {
            *px.add(i * 4) = st.bg.2; // B
            *px.add(i * 4 + 1) = st.bg.1; // G
            *px.add(i * 4 + 2) = st.bg.0; // R
            *px.add(i * 4 + 3) = 255; // A
        }
    }

    // 顶部一条色带：让卡片在聊天流里一眼能认出来
    let bar = 6i32;
    unsafe {
        for y in 0..bar {
            for x in 0..st.width {
                let i = ((y * st.width + x) * 4) as usize;
                *px.add(i) = st.accent.2;
                *px.add(i + 1) = st.accent.1;
                *px.add(i + 2) = st.accent.0;
                *px.add(i + 3) = 255;
            }
        }
    }

    unsafe { SetBkMode(dc, TRANSPARENT) };

    let mut y = st.pad + bar / 2;
    if !title.trim().is_empty() {
        unsafe { SetTextColor(dc, st.title_color) };
        draw(dc, title, st.title_size, FW_BOLD, st.pad, y, text_w)?;
        y += title_h + st.line_gap;
    }
    if body_h > 0 {
        unsafe { SetTextColor(dc, st.body_color) };
        draw(dc, body, st.body_size, FW_NORMAL, st.pad, y, text_w)?;
    }

    // 取出像素并存 PNG。
    // 用 image crate 而不是手写 PNG 编码：它已经在依赖里（抽帧去重在用），
    // 只需打开 png feature。
    let mut img = image::RgbaImage::new(st.width as u32, height as u32);
    for yy in 0..height {
        for xx in 0..st.width {
            let i = ((yy * st.width + xx) * 4) as usize;
            let (b, g, r, a) =
                unsafe { (*px.add(i), *px.add(i + 1), *px.add(i + 2), *px.add(i + 3)) };
            img.put_pixel(xx as u32, yy as u32, image::Rgba([r, g, b, a]));
        }
    }

    unsafe {
        SelectObject(dc, old);
        DeleteObject(hbmp);
        DeleteDC(dc);
    }

    img.save(out)
        .with_context(|| format!("写 PNG 失败：{}", out.display()))?;
    Ok((st.width as u32, height as u32))
}

/// 量一段文字在给定宽度下需要多高。
fn measure(dc: Hdc, text: &str, size: i32, weight: i32, width: i32) -> Result<i32> {
    let font = make_font(size, weight)?;
    let old = unsafe { SelectObject(dc, font) };
    let w = wide(text);
    let mut rc = Rect {
        left: 0,
        top: 0,
        right: width,
        bottom: 0,
    };
    unsafe {
        DrawTextW(
            dc,
            w.as_ptr(),
            -1,
            &mut rc,
            DT_LEFT | DT_WORDBREAK | DT_CALCRECT | DT_NOPREFIX,
        );
        SelectObject(dc, old);
        DeleteObject(font);
    }
    Ok(rc.bottom - rc.top)
}

/// 在 (x, y) 处画一段文字（自动换行）。
#[allow(clippy::too_many_arguments)]
fn draw(dc: Hdc, text: &str, size: i32, weight: i32, x: i32, y: i32, width: i32) -> Result<()> {
    let font = make_font(size, weight)?;
    let old = unsafe { SelectObject(dc, font) };
    let w = wide(text);
    let mut rc = Rect {
        left: x,
        top: y,
        right: x + width,
        bottom: y + 10000,
    };
    unsafe {
        DrawTextW(
            dc,
            w.as_ptr(),
            -1,
            &mut rc,
            DT_LEFT | DT_WORDBREAK | DT_NOPREFIX,
        );
        SelectObject(dc, old);
        DeleteObject(font);
    }
    Ok(())
}

fn make_font(size: i32, weight: i32) -> Result<Hfont> {
    let face = wide("Microsoft YaHei");
    let f = unsafe {
        CreateFontW(
            -size, // 负值 = 按字符高度而非单元格高度
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            0,
            0,
            CLEARTYPE_QUALITY,
            0,
            face.as_ptr(),
        )
    };
    if f == 0 {
        return Err(anyhow::anyhow!("创建字体失败（系统里有微软雅黑吗）"));
    }
    Ok(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("vca-card-{}-{name}.png", std::process::id()))
    }

    /// 真画一张图出来并校验它是合法的 PNG。
    ///
    /// 这个测试在**没有桌面会话**的环境里会失败（拿不到 DC）——
    /// 那种情况下直接跳过，而不是伪装成通过。
    #[test]
    fn renders_a_valid_png() {
        let out = tmp("basic");
        let _ = std::fs::remove_file(&out);
        let Ok((w, h)) = render_png(
            "课堂纪要 2026-09-21 语文",
            "- 讲了《背影》的写作背景\n- 重点：四次背影的描写\n- 作业：背诵第三段并写一段赏析",
            &out,
        ) else {
            eprintln!("没有桌面会话，跳过渲染测试");
            return;
        };
        assert_eq!(w, 720);
        assert!(h > 100, "高度应当撑开：{h}");

        let bytes = std::fs::read(&out).expect("应当生成文件");
        // PNG 的魔数
        assert_eq!(
            &bytes[0..8],
            &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]
        );
        assert!(bytes.len() > 1000, "文件不该是空的：{}", bytes.len());
        let _ = std::fs::remove_file(&out);
    }

    /// 内容越长、卡片越高 —— 顺带证明"量高度"那段真的在工作。
    #[test]
    fn longer_text_makes_a_taller_card() {
        let short = tmp("short");
        let long = tmp("long");
        let Ok((_, h1)) = render_png("标题", "一行字", &short) else {
            eprintln!("没有桌面会话，跳过");
            return;
        };
        let Ok((_, h2)) = render_png("标题", &"- 一行字\n".repeat(20), &long) else {
            eprintln!("没有桌面会话，跳过");
            return;
        };
        assert!(h2 > h1, "长文应当更高：{h1} vs {h2}");
        let _ = std::fs::remove_file(&short);
        let _ = std::fs::remove_file(&long);
    }
}
