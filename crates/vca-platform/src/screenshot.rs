//! 屏幕截图（GDI 实现，零第三方依赖）。
//!
//! 用途：录制期间按固定间隔抓一张桌面，供课后与课程段落关联。
//!
//! 为什么不用 ffmpeg 抽帧：截图是**高频小操作**，为它单独拉起一个 ffmpeg 进程不划算；
//! 直接用 GDI 的 `BitBlt` 抓屏并写成 BMP，几十毫秒即可完成，且不占用编码器。
//!
//! 输出格式为 BMP（无压缩，无需编码库）。

use std::ffi::c_void;

type Hdc = isize;
type Hbitmap = isize;
type Handle = isize;
type Bool = i32;

const SRCCOPY: u32 = 0x00CC_0020;
const BI_RGB: u32 = 0;
const DIB_RGB_COLORS: u32 = 0;

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

#[link(name = "user32")]
extern "system" {
    fn GetDC(hwnd: Handle) -> Hdc;
    fn ReleaseDC(hwnd: Handle, hdc: Hdc) -> i32;
    fn GetSystemMetrics(index: i32) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateCompatibleDC(hdc: Hdc) -> Hdc;
    fn DeleteDC(hdc: Hdc) -> Bool;
    fn CreateCompatibleBitmap(hdc: Hdc, w: i32, h: i32) -> Hbitmap;
    fn SelectObject(hdc: Hdc, obj: Handle) -> Handle;
    fn DeleteObject(obj: Handle) -> Bool;
    fn BitBlt(
        dst: Hdc,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        src: Hdc,
        sx: i32,
        sy: i32,
        rop: u32,
    ) -> Bool;
    fn GetDIBits(
        hdc: Hdc,
        hbm: Hbitmap,
        start: u32,
        lines: u32,
        bits: *mut c_void,
        bmi: *mut BitmapInfo,
        usage: u32,
    ) -> i32;
}

/// 抓屏失败原因。
#[derive(Debug, thiserror::Error)]
pub enum ShotError {
    /// 无图形会话或 GDI 调用失败。
    #[error("抓屏失败: {0}")]
    Capture(&'static str),
    /// 写文件失败。
    #[error("写入截图失败: {0}")]
    Io(String),
}

/// 抓取主屏并存为 24 位 BMP。
///
/// 返回 `(宽, 高)`。
pub fn capture_to_bmp(path: &std::path::Path) -> Result<(i32, i32), ShotError> {
    // SAFETY: 所有 GDI 句柄在本函数内创建并在返回前释放；
    // 像素缓冲区长度按 stride*height 精确分配。
    unsafe {
        let w = GetSystemMetrics(0);
        let h = GetSystemMetrics(1);
        if w <= 0 || h <= 0 {
            return Err(ShotError::Capture("拿不到屏幕尺寸"));
        }

        let screen = GetDC(0);
        if screen == 0 {
            return Err(ShotError::Capture("GetDC 失败"));
        }
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, w, h);
        if mem == 0 || bmp == 0 {
            let _ = ReleaseDC(0, screen);
            return Err(ShotError::Capture("创建内存 DC 失败"));
        }
        let old = SelectObject(mem, bmp);

        if BitBlt(mem, 0, 0, w, h, screen, 0, 0, SRCCOPY) == 0 {
            SelectObject(mem, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem);
            let _ = ReleaseDC(0, screen);
            return Err(ShotError::Capture("BitBlt 失败（可能无交互式桌面）"));
        }

        // 取像素（BMP 行按 4 字节对齐）
        let stride = ((w * 3 + 3) / 4) * 4;
        let mut pixels = vec![0u8; (stride * h) as usize];
        let mut bmi = BitmapInfo::default();
        bmi.header.size = std::mem::size_of::<BitmapInfoHeader>() as u32;
        bmi.header.width = w;
        bmi.header.height = h; // 正数 = 自下而上，正好是 BMP 的存储顺序
        bmi.header.planes = 1;
        bmi.header.bit_count = 24;
        bmi.header.compression = BI_RGB;

        let lines = GetDIBits(
            mem,
            bmp,
            0,
            h as u32,
            pixels.as_mut_ptr() as *mut c_void,
            &mut bmi,
            DIB_RGB_COLORS,
        );
        if lines == 0 {
            SelectObject(mem, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem);
            let _ = ReleaseDC(0, screen);
            return Err(ShotError::Capture("GetDIBits 失败"));
        }

        SelectObject(mem, old);
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(mem);
        let _ = ReleaseDC(0, screen);

        write_bmp(path, w, h, stride, &pixels)?;
        Ok((w, h))
    }
}

/// 写出 24 位 BMP。
fn write_bmp(
    path: &std::path::Path,
    w: i32,
    h: i32,
    stride: i32,
    pixels: &[u8],
) -> Result<(), ShotError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| ShotError::Io(e.to_string()))?;
    }
    let data_size = stride as u32 * h as u32;
    let file_size = 14 + 40 + data_size;

    let mut out: Vec<u8> = Vec::with_capacity(file_size as usize);
    // BITMAPFILEHEADER
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&file_size.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&(14u32 + 40).to_le_bytes()); // offset
                                                        // BITMAPINFOHEADER
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&data_size.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes()); // 72 DPI
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&pixels[..data_size as usize]);

    std::fs::write(path, out).map_err(|e| ShotError::Io(e.to_string()))?;
    Ok(())
}

/// 抓一张到 `dir` 下，文件名形如 `20250318_080000_shot001.bmp`。
pub fn capture_timed(
    dir: &std::path::Path,
    date: &str,
    time: &str,
    seq: u32,
) -> Result<std::path::PathBuf, ShotError> {
    let name = format!(
        "{}_{}_shot{:03}.bmp",
        date.replace('-', ""),
        time.replace(':', ""),
        seq
    );
    let path = dir.join(name);
    capture_to_bmp(&path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bmp_header_is_wellformed() {
        let dir = std::env::temp_dir().join(format!("vca_shot_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("t.bmp");
        let pixels = vec![0u8; 4 * 2 * 2]; // stride=4, h=2
        write_bmp(&p, 2, 2, 4, &pixels).unwrap();
        let bytes = std::fs::read(&p).unwrap();
        assert_eq!(&bytes[0..2], b"BM");
        // 数据区 = stride * height = 4 * 2 = 8 字节
        assert_eq!(bytes.len(), 14 + 40 + 8);
        // 偏移量
        let off = u32::from_le_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]);
        assert_eq!(off, 54);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn capture_does_not_panic_without_desktop() {
        // 无交互式桌面时可能失败，但绝不能 panic
        let dir = std::env::temp_dir().join(format!("vca_shot2_{}", std::process::id()));
        let r = capture_to_bmp(&dir.join("x.bmp"));
        let _ = std::fs::remove_dir_all(&dir);
        // 只要求返回 Result
        let _ = r;
    }
}
