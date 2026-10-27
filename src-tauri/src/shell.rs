//! 托盘与「短时提到前台」（DESIGN §5.3、不变项 #5）。
//!
//! 卡片平时属于桌面层，被普通应用正常盖住。托盘左键只是**短时**把它提到前台，
//! 到点必须自己把层级还回去——永久置顶是明确不做的事。

use std::sync::atomic::{AtomicU64, Ordering};

use crate::probe_log;
use tauri::menu::{Menu, MenuItemBuilder, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WebviewWindow};

/// 提到前台待多久。8 秒够看清卡片在不在、点两下，又不至于变成事实上的置顶。
pub const RAISE_MS: u64 = 8_000;

pub const TRAY_ID: &str = "perch-tray";
pub const ID_QUICK: &str = "quick-record";
pub const ID_VISIBLE: &str = "toggle-visible";
pub const ID_QUIT: &str = "quit";

/// 第几次「提到前台」。同一时刻只认最新那一次。
#[derive(Default)]
pub struct Raises(AtomicU64);

impl Raises {
    pub const fn new() -> Self {
        Raises(AtomicU64::new(0))
    }

    /// 开始一次提升，返回这一笔的代号。
    pub fn begin(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// 到点了要不要真的还回桌面层：中间又点过一次就不还，否则会把新的那次掐掉。
    pub fn is_current(&self, generation: u64) -> bool {
        self.0.load(Ordering::SeqCst) == generation
    }
}

static RAISES: Raises = Raises::new();

fn card(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("main")
}

/// 左键托盘：显示卡片并提到前台，`RAISE_MS` 后压回应用层最底。
pub fn raise_temporary(app: &AppHandle) {
    let Some(win) = card(app) else {
        return;
    };
    if !win.is_visible().unwrap_or(false) {
        let _ = win.show();
    }
    let _ = win.set_always_on_top(true);
    let _ = win.set_focus();
    let generation = RAISES.begin();
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(RAISE_MS));
        if !RAISES.is_current(generation) {
            return;
        }
        if let Some(win) = card(&handle) {
            let _ = win.set_always_on_top(false);
        }
        // 还层级只碰 Win32 的 z 序，不碰 Tauri 的窗口对象，所以可以在这个线程上做。
        unsafe { crate::desktop_layer::settle_bottom() };
    });
}

/// 显示/隐藏（托盘菜单那一项）。隐藏后进程还在，托盘左键能叫回来。
pub fn toggle_visible(app: &AppHandle) {
    let Some(win) = card(app) else {
        return;
    };
    if win.is_visible().unwrap_or(false) {
        let _ = win.hide();
    } else {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

pub fn build(app: &AppHandle) {
    // 快捷键可能被别的程序占了，托盘这项是它的兜底入口（§2.7、§5.3）。
    let quick = MenuItemBuilder::with_id(ID_QUICK, "快速记录").build(app);
    let toggle = MenuItemBuilder::with_id(ID_VISIBLE, "显示 / 隐藏卡片").build(app);
    let quit = MenuItemBuilder::with_id(ID_QUIT, "退出").build(app);
    let separator = PredefinedMenuItem::separator(app);
    let (quick, toggle, quit, separator) = match (quick, toggle, quit, separator) {
        (Ok(a), Ok(b), Ok(c), Ok(d)) => (a, b, c, d),
        _ => {
            probe_log::note("[tray] 建菜单失败，托盘没有挂上");
            return;
        }
    };
    let menu = match Menu::with_items(app, &[&quick, &toggle, &separator, &quit]) {
        Ok(menu) => menu,
        Err(e) => {
            probe_log::note(format!("[tray] 装菜单失败：{e}"));
            return;
        }
    };
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("栖迟")
        .on_menu_event(|app, event| match event.id().as_ref() {
            ID_QUICK => crate::quick::toggle(app),
            ID_VISIBLE => toggle_visible(app),
            ID_QUIT => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                raise_temporary(tray.app_handle());
            }
        });
    match app.default_window_icon() {
        Some(icon) => builder = builder.icon(icon.clone()),
        None => probe_log::note("[tray] 没有内置窗口图标，托盘只能建不出来"),
    }
    match builder.build(app) {
        Ok(_) => probe_log::note(
            "[tray] 托盘已挂上（左键短时提到前台，右键菜单：快速记录、显示/隐藏、退出）",
        ),
        Err(e) => probe_log::note(format!("[tray] 建托盘失败：{e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::Raises;

    /// 连点两次托盘：第一次的定时器不能把第二次刚提上来的卡片压回桌面层。
    #[test]
    fn only_the_latest_raise_gives_the_layer_back() {
        let raises = Raises::default();
        let first = raises.begin();
        let second = raises.begin();
        assert_ne!(first, second);
        assert!(!raises.is_current(first), "过期的一次不能再还层级");
        assert!(raises.is_current(second));
    }

    /// 没有并发时每次都是最新一次，层级该还得还——不然就成了永久置顶（不变项 #5）。
    #[test]
    fn a_single_raise_is_always_current() {
        let raises = Raises::new();
        for _ in 0..3 {
            let generation = raises.begin();
            assert!(raises.is_current(generation));
        }
    }
}
