//! 用 PrintWindow 把窗口自身画进位图，绕开「屏幕上被别的窗口挡住」的干扰。
//! 只在 spike 阶段用来判断 WebView2 有没有真的渲染出内容。

use std::ffi::c_void;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, DIB_RGB_COLORS, GetDC,
    ReleaseDC, SelectObject, SRCCOPY, BITMAPINFO, BITMAPINFOHEADER, RGBQUAD,
};
use windows_sys::Win32::Storage::Xps::PrintWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowRect, PW_RENDERFULLCONTENT};

/// 抓一张窗口自身的内容，存成 BMP，并返回一行可直接写日志的结论。
pub unsafe fn capture(hwnd: HWND, out: &Path) -> String {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if GetWindowRect(hwnd, &mut r) == 0 {
        return "GetWindowRect 失败".to_string();
    }
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if w <= 0 || h <= 0 {
        return format!("窗口尺寸非法 {w}x{h}");
    }

    let screen_dc = GetDC(std::ptr::null_mut());
    let mem_dc = CreateCompatibleDC(screen_dc);
    if mem_dc.is_null() {
        ReleaseDC(std::ptr::null_mut(), screen_dc);
        return "CreateCompatibleDC 失败".to_string();
    }
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: 0,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [RGBQUAD {
            rgbBlue: 0,
            rgbGreen: 0,
            rgbRed: 0,
            rgbReserved: 0,
        }],
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    let bmp = CreateDIBSection(mem_dc, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
    if bmp.is_null() || bits.is_null() {
        DeleteDC(mem_dc);
        ReleaseDC(std::ptr::null_mut(), screen_dc);
        return "CreateDIBSection 失败".to_string();
    }
    let old = SelectObject(mem_dc, bmp as *mut c_void);
    let drew = PrintWindow(hwnd, mem_dc, PW_RENDERFULLCONTENT);

    let pixels = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
    let stats = pixel_stats(pixels);
    let written = write_bmp(out, w, h, pixels).map(|_| String::new()).unwrap_or_else(|e| format!(" 写文件失败 {e}"));

    SelectObject(mem_dc, old);
    DeleteObject(bmp as *mut c_void);
    DeleteDC(mem_dc);
    ReleaseDC(std::ptr::null_mut(), screen_dc);

    format!(
        "PrintWindow={drew} {w}x{h} → {}{}{stats}",
        out.display(),
        written
    )
}

/// 抓屏幕上一块真实合成后的像素。PrintWindow 只能证明「窗口自己画了」，
/// 能不能在桌面上看见要靠这个——透明窗口挂进 WorkerW 后有可能整个不再合成。
pub unsafe fn screen(x: i32, y: i32, w: i32, h: i32, out: &Path) -> String {
    if w <= 0 || h <= 0 {
        return format!("抓屏尺寸非法 {w}x{h}");
    }
    let screen_dc = GetDC(std::ptr::null_mut());
    let mem_dc = CreateCompatibleDC(screen_dc);
    if mem_dc.is_null() {
        ReleaseDC(std::ptr::null_mut(), screen_dc);
        return "CreateCompatibleDC 失败".to_string();
    }
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: 0,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [RGBQUAD {
            rgbBlue: 0,
            rgbGreen: 0,
            rgbRed: 0,
            rgbReserved: 0,
        }],
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    let bmp = CreateDIBSection(mem_dc, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
    if bmp.is_null() || bits.is_null() {
        DeleteDC(mem_dc);
        ReleaseDC(std::ptr::null_mut(), screen_dc);
        return "CreateDIBSection 失败".to_string();
    }
    let old = SelectObject(mem_dc, bmp as *mut c_void);
    let blted = BitBlt(
        mem_dc,
        0,
        0,
        w,
        h,
        screen_dc,
        x,
        y,
        SRCCOPY,
    );
    let pixels = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
    let written = write_bmp(out, w, h, pixels)
        .map(|_| String::new())
        .unwrap_or_else(|e| format!(" 写文件失败 {e}"));
    // 结论必须在 DeleteObject 之前算完：像素内存属于那个 DIB，删掉就没有了。
    let report = format!(
        "BitBlt={blted} 屏幕({x},{y}) {w}x{h} → {}{written}；中心 {}｜左上角 {}",
        out.display(),
        patch_avg(pixels, w, h, w / 2, h / 2),
        patch_avg(pixels, w, h, 10, 10)
    );
    SelectObject(mem_dc, old);
    DeleteObject(bmp as *mut c_void);
    DeleteDC(mem_dc);
    ReleaseDC(std::ptr::null_mut(), screen_dc);
    report
}

/// 以 (cx, cy) 为中心取 24x24 的平均色，用来分辨「卡片本体」和「卡片外的壁纸」。
fn patch_avg(px: &[u8], w: i32, _h: i32, cx: i32, cy: i32) -> String {
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    for dy in -12..12 {
        for dx in -12..12 {
            let (x, y) = (cx + dx, cy + dy);
            if x < 0 || y < 0 || x >= w {
                continue;
            }
            let i = ((y * w + x) * 4) as usize;
            if i + 3 >= px.len() {
                continue;
            }
            b += px[i] as u64;
            g += px[i + 1] as u64;
            r += px[i + 2] as u64;
            n += 1;
        }
    }
    if n == 0 {
        return "无像素".to_string();
    }
    format!("#{:02x}{:02x}{:02x}", r / n, g / n, b / n)
}

/// 只统计「有没有东西画上来了」：非全黑且不透明的像素占比，以及平均色。
fn pixel_stats(px: &[u8]) -> String {
    let mut lit = 0usize;
    let (mut sr, mut sg, mut sb) = (0u64, 0u64, 0u64);
    let n = px.len() / 4;
    for chunk in px.chunks_exact(4) {
        let (b, g, r, a) = (chunk[0], chunk[1], chunk[2], chunk[3]);
        if a > 0 && (r > 8 || g > 8 || b > 8) {
            lit += 1;
            sr += r as u64;
            sg += g as u64;
            sb += b as u64;
        }
    }
    if lit == 0 {
        return "空白（没有任何像素画上来了）".to_string();
    }
    format!(
        "有内容像素 {:.1}% 平均色≈#{:02x}{:02x}{:02x}",
        lit as f64 * 100.0 / n as f64,
        sr / lit as u64,
        sg / lit as u64,
        sb / lit as u64
    )
}

fn write_bmp(path: &Path, w: i32, h: i32, px: &[u8]) -> std::io::Result<()> {
    let stride = w as usize * 4;
    let row_bytes = stride * h as usize;
    let file_size = 14 + 40 + row_bytes;
    let mut f = BufWriter::new(File::create(path)?);
    f.write_all(b"BM")?;
    f.write_all(&(file_size as u32).to_le_bytes())?;
    f.write_all(&0u32.to_le_bytes())?;
    f.write_all(&(14u32 + 40u32).to_le_bytes())?;
    f.write_all(&40u32.to_le_bytes())?;
    f.write_all(&w.to_le_bytes())?;
    f.write_all(&h.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&32u16.to_le_bytes())?;
    f.write_all(&0u32.to_le_bytes())?;
    f.write_all(&(row_bytes as u32).to_le_bytes())?;
    f.write_all(&0u32.to_le_bytes())?;
    f.write_all(&0u32.to_le_bytes())?;
    f.write_all(&0u32.to_le_bytes())?;
    f.write_all(&0u32.to_le_bytes())?;
    // 内存里是自上而下，BMP 标准是自下而上，所以按行倒序落盘。
    // （写成负高度的话，看图软件各家解释不一致，实测 Pillow 就会翻。）
    for row in px.chunks_exact(stride).rev() {
        f.write_all(row)?;
    }
    f.flush()
}
