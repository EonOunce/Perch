//! 快速记录小窗（DESIGN §2.7、§5.3）。
//!
//! 独立窗口、临时置顶、不进任务栏；主卡片的层级与状态都不受影响——
//! 「桌面层不是置顶」这条只对卡片成立，小窗是用户主动叫出来的，理应当压在最上面。
//!
//! 第一次打开才建窗口，之后只隐藏不销毁：重建一个 WebView2 要几百毫秒，
//! 而「按下快捷键马上能写」是这个小窗存在的全部理由。

use crate::probe_log;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub const LABEL: &str = "quick";
pub const WIDTH: f64 = 380.0;
pub const HEIGHT: f64 = 232.0;

/// 居中偏上（§5.3）：横向正中，纵向落在工作区上沿往下 18% 处。
/// 传进来的是**逻辑**尺寸，返回值也是逻辑偏移，方便按同一套口径测试。
pub fn spot(mw: f64, mh: f64, w: f64, h: f64) -> (f64, f64) {
    let x = ((mw - w) / 2.0).max(0.0);
    // 「偏上」取上沿往下 18%，但屏幕比窗口高不了多少时贴顶，不许跑到工作区外面。
    (x, (mh * 0.18).clamp(0.0, (mh - h).max(0.0)))
}

/// 打开或收起小窗。快捷键、托盘菜单、日期栏的笔形按钮都走这一条路径。
///
/// 建窗与显示都要排回主线程：tao 的事件循环在主线程，从快捷键线程直接建出来的窗口
/// 虽然能显示、能成为前台，却收不到任何 `keydown`（2026-10-28 真机实测）。
pub fn toggle(app: &AppHandle) {
    on_main(app, toggle_now);
}

pub fn hide(app: &AppHandle) {
    on_main(app, move |app| {
        if let Some(win) = app.get_webview_window(LABEL) {
            let _ = win.hide();
        }
    });
}

fn on_main(app: &AppHandle, task: impl FnOnce(&AppHandle) + Send + 'static) {
    let handle = app.clone();
    if let Err(e) = app.run_on_main_thread(move || task(&handle)) {
        probe_log::note(format!("[quick] 主线程没接活：{e}"));
    }
}

fn toggle_now(app: &AppHandle) {
    match app.get_webview_window(LABEL) {
        Some(win) => set_visible(app, &win, !win.is_visible().unwrap_or(false)),
        None => match build(app) {
            Ok(win) => set_visible(app, &win, true),
            Err(e) => probe_log::note(format!("[quick] 建小窗失败：{e}")),
        },
    }
}

fn build(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("quick.html".into()))
        .title("栖迟 · 快速记录")
        .inner_size(WIDTH, HEIGHT)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .build()
}

fn set_visible(app: &AppHandle, win: &WebviewWindow, show: bool) {
    if !show {
        let _ = win.hide();
        return;
    }
    place(app, win);
    let _ = win.show();
    let _ = win.set_focus();
}

/// 每次显示都重新定位：显示器换了、缩放改了、上次被拖到别处，都以「居中偏上」为准。
fn place(_app: &AppHandle, win: &WebviewWindow) {
    let scale = win.scale_factor().unwrap_or(1.0);
    let monitor = win.current_monitor().ok().flatten();
    let Some(monitor) = monitor else {
        let _ = win.center();
        return;
    };
    let size = monitor.size();
    let origin = monitor.position();
    let (x, y) = spot(
        size.width as f64 / scale,
        size.height as f64 / scale,
        WIDTH,
        HEIGHT,
    );
    let _ = win.set_position(tauri::Position::Physical(tauri::PhysicalPosition {
        x: (origin.x as f64 + x * scale).round() as i32,
        y: (origin.y as f64 + y * scale).round() as i32,
    }));
}

#[tauri::command]
pub fn quick_toggle(app: AppHandle) {
    toggle(&app);
}

#[tauri::command]
pub fn quick_hide(app: AppHandle) {
    hide(&app);
}

/// 存一条碎碎念，再让主卡片刷新。
/// 小窗和卡片是两个 webview，卡片不会自己知道数据变了（§2.7「不改变主卡片状态」
/// 说的是层级与折叠档，内容当然要跟上新记录）。
#[tauri::command]
pub fn quick_save(app: AppHandle, text: String) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("空内容不保存".into());
    }
    let state = app.state::<crate::store::SharedState>();
    let _ = crate::commands::note_add(text.to_string(), state)?;
    let _ = app.emit("perch:changed", ());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{spot, HEIGHT, WIDTH};

    /// 1920×1080 上：横向正中、纵向在上四分之一处，且整窗都在屏内。
    #[test]
    fn the_quick_window_lands_centred_and_upper() {
        let (x, y) = spot(1920.0, 1080.0, WIDTH, HEIGHT);
        assert_eq!(x, (1920.0 - WIDTH) / 2.0);
        assert!(
            (y - 1080.0 * 0.18).abs() < 0.01,
            "取工作区高度的 18%，实际 {y}"
        );
        assert!(x > 0.0 && y > 0.0);
        assert!(
            x + WIDTH <= 1920.0 && y + HEIGHT <= 1080.0,
            "不能跑到屏幕外"
        );
    }

    /// 屏幕比窗口还小时不出现负坐标——负坐标会把窗口推到工作区外面看不见。
    #[test]
    fn a_screen_smaller_than_the_window_stays_at_the_origin() {
        assert_eq!(spot(200.0, 120.0, WIDTH, HEIGHT), (0.0, 0.0));
    }

    /// 只比窗口高一点的窄屏：18% 会把下沿挤出屏幕，这时贴顶而不是溢出。
    #[test]
    fn a_short_screen_pins_it_to_the_top_instead_of_overflowing() {
        let mh = HEIGHT + 20.0;
        let (x, y) = spot(1920.0, mh, WIDTH, HEIGHT);
        assert_eq!(x, (1920.0 - WIDTH) / 2.0);
        assert_eq!(y, 20.0, "18% 会挤出下沿，只能贴到 mh - h");
        assert!(y + HEIGHT <= mh, "整窗必须还在屏内");
    }
}
