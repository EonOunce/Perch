//! 全局快捷键 `Ctrl+Alt+N`（DESIGN §5.3）。
//!
//! Tauri 的全局快捷键在独立插件里，为它要新增一条依赖链；`RegisterHotKey` 是
//! windows-sys 已经带上的系统调用，配一个消息专用窗口就能在自己的线程里收到 WM_HOTKEY，
//! 不必碰主线程，也不给离线构建添新包。
//!
//! 注册失败（快捷键被别的程序占了）只记日志并给卡片一句提示，托盘与卡片内的入口照常在。

use std::sync::OnceLock;

use crate::probe_log;
use tauri::{AppHandle, Emitter};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, VK_N,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassExW,
    TranslateMessage, CS_HREDRAW, HWND_MESSAGE, MSG, WM_HOTKEY, WNDCLASSEXW, WS_POPUP,
};

const HOTKEY_ID: i32 = 0x5045;
const CLASS: &str = "PerchHotkey";

static APP: OnceLock<AppHandle> = OnceLock::new();

/// `Ctrl+Alt+N` 的原始参数（修饰键、虚拟键码）。
/// `MOD_NOREPEAT`：按住不放不会连发，免得一次按键弹出又关掉小窗。
pub fn quick_record_key() -> (u32, u32) {
    (MOD_ALT | MOD_CONTROL | MOD_NOREPEAT, VK_N as u32)
}

pub fn spawn(app: AppHandle) {
    if APP.set(app.clone()).is_err() {
        probe_log::note("[hotkey] 已经在监听，不重复起线程");
        return;
    }
    std::thread::spawn(|| unsafe {
        let hinstance = GetModuleHandleW(std::ptr::null());
        let name = wide(CLASS);
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: name.as_ptr(),
            hIconSm: std::ptr::null_mut(),
        };
        if RegisterClassExW(&class) == 0 {
            probe_log::note("[hotkey] 注册窗口类失败，快捷键没装");
            return;
        }
        // 消息专用窗口：只收消息，不在屏上、不进 alt-tab。
        let hwnd = CreateWindowExW(
            0,
            name.as_ptr(),
            name.as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            probe_log::note("[hotkey] 建消息窗口失败，快捷键没装");
            return;
        }
        let (mods, vk) = quick_record_key();
        if RegisterHotKey(hwnd, HOTKEY_ID, mods, vk) == 0 {
            probe_log::note(
                "[hotkey] Ctrl+Alt+N 注册失败（多半被别的程序占了），改用托盘/卡片入口",
            );
            if let Some(app) = APP.get() {
                let _ = app.emit(
                    "perch:hint",
                    "Ctrl+Alt+N 被别的程序占用了，快速记录请走托盘或日期栏的笔形按钮",
                );
            }
            return;
        }
        probe_log::note("[hotkey] Ctrl+Alt+N 已注册");
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_HOTKEY && wparam == HOTKEY_ID as usize {
        if let Some(app) = APP.get() {
            crate::quick::toggle(app);
        }
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::quick_record_key;

    /// 快捷键口径写死在测试里：改了要同时改 DESIGN §5.3 与关于页。
    #[test]
    fn the_quick_record_shortcut_is_ctrl_alt_n() {
        let (mods, vk) = quick_record_key();
        assert_eq!(mods, 1 | 2 | 16384, "MOD_ALT | MOD_CONTROL | MOD_NOREPEAT");
        assert_eq!(vk, 78, "VK_N");
    }
}
