//! 托盘与「短时提到前台」（DESIGN §5.3、不变项 #5）。
//!
//! 卡片平时属于桌面层，被普通应用正常盖住。托盘左键只是**短时**把它提到前台，
//! 到点必须自己把层级还回去——永久置顶是明确不做的事。

use std::sync::atomic::{AtomicU64, Ordering};

use crate::probe_log;
use tauri::menu::{CheckMenuItem, Menu, MenuItemBuilder, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow, Wry};

/// 提到前台待多久。8 秒够看清卡片在不在、点两下，又不至于变成事实上的置顶。
pub const RAISE_MS: u64 = 8_000;

pub const TRAY_ID: &str = "perch-tray";
pub const ID_QUICK: &str = "quick-record";
pub const ID_VISIBLE: &str = "toggle-visible";
pub const ID_SYNC_NOW: &str = "sync-now";
pub const ID_SYNC_SETTINGS: &str = "sync-settings";
pub const ID_ABOUT: &str = "about";
pub const ID_AUTOSTART: &str = "autostart";
pub const ID_MORNING: &str = "morning-raise";
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
    raise(app, true);
}

/// 早晨自动浮出：同样只提一小会儿，但**不抢键盘焦点**（§2.3「优先不打断正在输入的用户」）。
pub fn raise_quiet(app: &AppHandle) {
    raise(app, false);
}

fn raise(app: &AppHandle, take_focus: bool) {
    let Some(win) = card(app) else {
        return;
    };
    if !win.is_visible().unwrap_or(false) {
        let _ = win.show();
    }
    let _ = win.set_always_on_top(true);
    if take_focus {
        let _ = win.set_focus();
    }
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

/// 托盘「关于栖迟」（§2.10 的第二个入口）。卡片本来可能被藏着或压在桌面层，
/// 所以先按左键那条路把它短时提到前台，再让界面自己把面板打开——层级仍会到点还回去。
pub fn show_about(app: &AppHandle) {
    raise_temporary(app);
    let _ = app.emit("perch:about", ());
}

/// 托盘「立即同步」：配好了就排一轮；没配好就把设置打开——点了一项却什么也没发生，
/// 比少一项更糟（§5.3）。
fn sync_now(app: &AppHandle) {
    if crate::sync::view(app).configured {
        app.state::<crate::sync::Syncer>().request_manual();
        probe_log::note("[tray] 已排入一轮手动同步");
    } else {
        show_sync_settings(app);
    }
}

/// 同步设置面板走的还是「先短时提层、再让界面打开」那一条（§2.10）。
fn show_sync_settings(app: &AppHandle) {
    raise_temporary(app);
    let _ = app.emit("perch:sync-settings", ());
}

/// 托盘菜单里两个「自己会变的项」的文案。
///
/// Win11 的托盘弹层不给 `CheckMenuItem` 画对勾（实测 UIA 报 `toggle=On` 但界面上看不出来），
/// 所以把状态直接写进文字；勾选位仍然保留，读屏软件靠它报状态。
pub fn autostart_label(autostart: bool) -> &'static str {
    if autostart {
        "开机自启（已开启）"
    } else {
        "开机自启"
    }
}

/// 早晨浮出。设置页还没做，所以「可关」这一条暂时挂在托盘菜单上（§2.3、§5.3）。
pub fn morning_label(on: bool) -> &'static str {
    if on {
        "早晨浮出（已开启）"
    } else {
        "早晨浮出"
    }
}

/// 托盘菜单的整份内容。会变的项每次现读设置，不在别处另存一份。
fn menu_of(app: &AppHandle, autostart: bool, morning: bool) -> tauri::Result<Menu<Wry>> {
    // 快捷键可能被别的程序占了，托盘这项是它的兜底入口（§2.7、§5.3）。
    let quick = MenuItemBuilder::with_id(ID_QUICK, "快速记录").build(app)?;
    let toggle = MenuItemBuilder::with_id(ID_VISIBLE, "显示 / 隐藏卡片").build(app)?;
    // 手动同步与设置。没配好时「立即同步」直接打开设置，免得点了一个什么也不会发生的项。
    let sync_now = MenuItemBuilder::with_id(ID_SYNC_NOW, "立即同步").build(app)?;
    let sync_settings = MenuItemBuilder::with_id(ID_SYNC_SETTINGS, "同步设置").build(app)?;
    // 说明的唯一系统级入口（§2.10）：卡片右上角那个 ⓘ 之外的第二个入口。
    let about = MenuItemBuilder::with_id(ID_ABOUT, "关于栖迟").build(app)?;
    // 默认关，所以第一次看到的就是没有「已开启」三个字（§12 #6）。
    let startup = CheckMenuItem::with_id(
        app,
        ID_AUTOSTART,
        autostart_label(autostart),
        true,
        autostart,
        None::<&str>,
    )?;
    let dawn = CheckMenuItem::with_id(
        app,
        ID_MORNING,
        morning_label(morning),
        true,
        morning,
        None::<&str>,
    )?;
    let quit = MenuItemBuilder::with_id(ID_QUIT, "退出").build(app)?;
    let separator = PredefinedMenuItem::separator(app)?;
    Menu::with_items(
        app,
        &[
            &quick,
            &toggle,
            &sync_now,
            &sync_settings,
            &about,
            &startup,
            &dawn,
            &separator,
            &quit,
        ],
    )
}

pub fn build(app: &AppHandle) {
    let menu = match menu_of(
        app,
        crate::autostart::is_on(app),
        crate::morning::is_on(app),
    ) {
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
            ID_SYNC_NOW => sync_now(app),
            ID_SYNC_SETTINGS => show_sync_settings(app),
            ID_ABOUT => show_about(app),
            ID_AUTOSTART => crate::autostart::toggle(app),
            ID_MORNING => crate::morning::toggle(app),
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
            "[tray] 托盘已挂上（左键短时提到前台，右键菜单：快速记录、显示/隐藏、立即同步、同步设置、关于栖迟、开机自启、早晨浮出、退出）",
        ),
        Err(e) => probe_log::note(format!("[tray] 建托盘失败：{e}")),
    }
}

/// 菜单里那两项的状态变了就重挂一遍。只有这两项会自己变，
/// 用「整份重建」而不是记住某几个菜单项：重建只在建菜单时碰主线程，句柄不用跨线程存着。
pub fn refresh_menu(app: &AppHandle) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        probe_log::note("[tray] 刷新菜单时找不到托盘");
        return;
    };
    let result = menu_of(
        app,
        crate::autostart::is_on(app),
        crate::morning::is_on(app),
    )
    .map_err(|e| e.to_string())
    .and_then(|menu| tray.set_menu(Some(menu)).map_err(|e| e.to_string()));
    match result {
        Ok(()) => probe_log::note("[tray] 菜单里两项的开启状态已刷新"),
        Err(e) => probe_log::note(format!("[tray] 刷新菜单失败：{e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::{autostart_label, morning_label, Raises};

    /// 开着和关着必须一眼能分出来：托盘弹层不画对勾，状态只能靠文字。
    #[test]
    fn the_autostart_item_says_whether_it_is_on() {
        assert_eq!(autostart_label(false), "开机自启");
        assert_eq!(autostart_label(true), "开机自启（已开启）");
    }

    /// 早晨浮出默认是开的，所以第一次看托盘就该看见「已开启」——不能反过来骗人。
    #[test]
    fn the_morning_item_says_whether_it_is_on() {
        assert_eq!(morning_label(false), "早晨浮出");
        assert_eq!(morning_label(true), "早晨浮出（已开启）");
    }

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
