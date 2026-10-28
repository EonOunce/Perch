#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod capture;
mod card_view;
mod clock;
mod commands;
mod credential;
mod desktop_layer;
mod export;
mod hotkey;
mod morning;
mod place;
mod probe_log;
mod quick;
mod shell;
mod store;
mod sync;
mod webdav;

use std::ffi::c_void;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewWindow};
use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW;

const PROBE_TITLE: &str = "栖迟";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 探针用取 HWND 的最省事办法：按窗口标题找。正式实现走 Tauri 的 hwnd()。
fn find_card() -> Option<*mut c_void> {
    let title = wide(PROBE_TITLE);
    let hwnd = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
    if hwnd.is_null() {
        None
    } else {
        Some(hwnd)
    }
}

/// 把卡片放到当前显示器工作区右下角，留 margin 物理像素边距。
fn place_bottom_right(win: &WebviewWindow, margin: i32) {
    let Some(monitor) = win.current_monitor().ok().flatten() else {
        probe_log::note("[place] 拿不到显示器信息，保持配置文件里的坐标");
        return;
    };
    let area = monitor.work_area();
    let size = win.inner_size().unwrap_or(PhysicalSize::new(360, 480));
    // `set_position` 挪的是窗口外框，而无边框窗口仍有一圈看不见的调整边框：不把它折算掉，
    // 内容区的右下角就会比设定的边距多出这么宽。
    let outer = win.outer_position().unwrap_or(PhysicalPosition::new(0, 0));
    let inner = win.inner_position().unwrap_or(PhysicalPosition::new(0, 0));
    let border = (inner.x - outer.x, inner.y - outer.y);
    let x = area.position.x + area.size.width as i32 - size.width as i32 - margin - border.0;
    let y = area.position.y + area.size.height as i32 - size.height as i32 - margin - border.1;
    probe_log::note(format!(
        "[place] 工作区原点=({},{}) 尺寸={}x{} → 卡片 ({},{}) {}x{}（logical 配置 360x480，缩放={:.2}）",
        area.position.x,
        area.position.y,
        area.size.width,
        area.size.height,
        x,
        y,
        size.width,
        size.height,
        size.height as f64 / 480.0
    ));
    if let Err(e) = win.set_position(PhysicalPosition::new(x, y)) {
        probe_log::note(format!("[place] set_position 失败: {e}"));
    }
}

/// 开机时窗口先按记住的折叠状态挑好尺寸档，再摆位置（§2.1、§2.8）。
fn boot_slot(settings: &store::Settings) -> &'static str {
    match (settings.collapsed, settings.strip_rows) {
        (false, _) => "today",
        (true, true) => "collapsed",
        (true, false) => "bar",
    }
}

fn work_rect(monitor: &tauri::Monitor) -> place::Rect {
    let area = monitor.work_area();
    place::Rect {
        x: area.position.x,
        y: area.position.y,
        width: area.size.width,
        height: area.size.height,
    }
}

/// 窗口左上角落在哪块屏上。跨屏拖动时左上角可能已经在工作区外，
/// 退回系统报的「最近显示器」，再不行用第一块。
fn monitor_at(win: &WebviewWindow, pos: (i32, i32)) -> Option<tauri::Monitor> {
    let monitors = win.available_monitors().ok()?;
    let containing = monitors.iter().find(|monitor| {
        let area = monitor.work_area();
        pos.0 >= area.position.x
            && pos.0 < area.position.x + area.size.width as i32
            && pos.1 >= area.position.y
            && pos.1 < area.position.y + area.size.height as i32
    });
    containing
        .cloned()
        .or_else(|| win.current_monitor().ok().flatten())
        .or_else(|| monitors.first().cloned())
}

/// 把当前物理位置折成「显示器 + 相对逻辑坐标」再记下来（§3.4）。
fn place_of(win: &WebviewWindow, pos: (i32, i32)) -> Option<store::Place> {
    let monitor = monitor_at(win, pos)?;
    let (x, y) = place::to_relative(work_rect(&monitor), monitor.scale_factor(), pos);
    Some(store::Place {
        monitor: monitor.name().cloned(),
        x,
        y,
    })
}

/// 记住的位置换算回物理像素。显示器不在了就落在第一块屏，
/// 换算完再夹回工作区，整张卡始终看得见（place.rs）。
fn saved_position(
    win: &WebviewWindow,
    settings: &store::Settings,
) -> Option<PhysicalPosition<i32>> {
    let place = settings.place.as_ref()?;
    let monitors = win.available_monitors().ok()?;
    let monitor = place
        .monitor
        .as_deref()
        .and_then(|name| {
            monitors
                .iter()
                .find(|monitor| monitor.name().map(String::as_str) == Some(name))
        })
        .or_else(|| monitors.first())?
        .clone();
    let size = win.outer_size().ok()?;
    let (x, y) = place::from_relative(
        work_rect(&monitor),
        monitor.scale_factor(),
        (size.width, size.height),
        (place.x, place.y),
    );
    Some(PhysicalPosition::new(x, y))
}

fn remember_position(app: &AppHandle, pos: PhysicalPosition<i32>) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let Some(place) = place_of(&win, (pos.x, pos.y)) else {
        return;
    };
    let prefs = app.state::<store::SharedPrefs>();
    if let Err(err) = prefs.with(|prefs| prefs.update(|settings| settings.place = Some(place))) {
        probe_log::note(format!("[place] 记住位置失败：{err}"));
    }
}

/// 拖动时每次 Moved 只把代号加一；等位置连续 250ms 没再变，
/// 保存线程才去读一次真实坐标写盘，拖动过程中不会每像素写一个文件。
struct MoveSignal {
    state: Mutex<MoveState>,
    settle: Condvar,
}

#[derive(Default)]
struct MoveState {
    generation: u64,
    saved: u64,
}

impl MoveSignal {
    fn new() -> Self {
        MoveSignal {
            state: Mutex::new(MoveState::default()),
            settle: Condvar::new(),
        }
    }

    fn bump(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.generation += 1;
        self.settle.notify_all();
    }

    /// 等位置静下来，返回这一轮该落盘的代号。
    fn quiet(&self) -> u64 {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            while state.generation == state.saved {
                state = self
                    .settle
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            let target = state.generation;
            let (next, _) = self
                .settle
                .wait_timeout(state, Duration::from_millis(250))
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state = next;
            if state.generation == target {
                return target;
            }
        }
    }

    fn saved(&self, generation: u64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.saved = state.saved.max(generation);
    }
}

fn start_position_saver(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || loop {
        let generation = handle.state::<MoveSignal>().quiet();
        if let Some(win) = handle.get_webview_window("main") {
            if let Ok(pos) = win.outer_position() {
                remember_position(&handle, pos);
            }
        }
        handle.state::<MoveSignal>().saved(generation);
    });
}

/// 退出前补记一次：拖动刚结束就关掉应用时，防抖那次可能还没等到。
fn flush_position(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        if let Ok(pos) = win.outer_position() {
            remember_position(app, pos);
        }
    }
}

/// 界面画完第一帧再露面（窗口在配置里是 visible:false）：
/// 折叠用户不该每次开机先看见一眼展开的卡片。
#[tauri::command]
fn ui_ready(app: AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        if let Err(err) = win.show() {
            probe_log::note(format!("[boot] 显示窗口失败：{err}"));
        }
        hide_window_border(&win);
    }
}

/// 立刻走一遍早晨浮出的判断：不等启动那一拍，也不复制一份判断逻辑。
/// 要量的是「提层有没有把她正在用的窗口挤到后台」，那只能对着前台句柄测。
#[tauri::command]
fn probe_morning(app: AppHandle) -> &'static str {
    if std::env::var_os("PERCH_PROBE").is_none() {
        return "探针未开启";
    }
    morning::decide(&app);
    "判断已走一遍"
}

/// 界面脚本要是没起来，3 秒后也把窗口露出来——看不见的卡片比一句报错更难查。
fn spawn_show_watchdog(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        if let Some(win) = handle.get_webview_window("main") {
            if !win.is_visible().unwrap_or(false) {
                probe_log::note("[boot] 界面 3 秒没报到，先把窗口显示出来");
                let _ = win.show();
                hide_window_border(&win);
            }
        }
    });
}

/// 去掉 DWM 沿窗口矩形画的那圈直角边框线（卡片是 22px 圆角，线会露在圆角外面）。
/// 只改 DWM 属性、不动窗口样式位：动样式位会把非客户区画成白框并露出关闭按钮。
fn hide_window_border(win: &WebviewWindow) {
    if let Ok(hwnd) = win.hwnd() {
        unsafe { desktop_layer::hide_dwm_border(hwnd.0) };
        schedule_frame_extend(hwnd.0);
    }
}

/// 顶部那 1px 标题栏残线只能靠扩展客户区去掉，而扩展必须等合成稳定之后调，
/// 否则整窗在屏上消失（见 desktop_layer::extend_frame 的注释）。
fn schedule_frame_extend(hwnd: *mut c_void) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    // 裸指针不是 Send，过线程边界先换成 usize。
    let raw = hwnd as usize;
    ONCE.call_once(|| {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1500));
            unsafe { desktop_layer::extend_frame(raw as *mut c_void) };
        });
    });
}

#[tauri::command]
fn probe_ping() -> String {
    format!("pong {}", probe_log::stamp())
}

#[tauri::command]
fn probe_status() -> String {
    unsafe { desktop_layer::status_line() }
}

#[tauri::command]
fn probe_embed() -> &'static str {
    unsafe { desktop_layer::embed(desktop_layer::DEFAULT_MARGIN).label() }
}

#[tauri::command]
fn probe_unembed() -> &'static str {
    unsafe {
        desktop_layer::unembed();
    }
    "已还原"
}

#[tauri::command]
fn probe_cycle() -> &'static str {
    unsafe { desktop_layer::cycle() }
}

/// 自己广播一次 TaskbarCreated，等价于「Explorer 重启后」桌面窗口全部换了一遍。
#[tauri::command]
fn probe_shell_event() -> &'static str {
    desktop_layer::poke_shell_event()
}

/// 把窗口往右下挪 20 物理像素：验证嵌入后是否仍能移动、以及两种移动路径的差别。
#[tauri::command]
fn probe_shift(window: WebviewWindow) -> String {
    let before = window
        .outer_position()
        .map(|p| (p.x, p.y))
        .unwrap_or((0, 0));
    let after = (before.0 + 20, before.1 + 20);
    match window.set_position(PhysicalPosition::new(after.0, after.1)) {
        Ok(()) => format!(
            "Tauri set_position {before:?} → {after:?}；实际 {:?}",
            window.outer_position().map(|p| (p.x, p.y))
        ),
        Err(e) => format!("移动失败: {e}"),
    }
}

#[tauri::command]
fn probe_quit(app: AppHandle) {
    app.exit(0);
}

/// 托盘菜单那几项的探针入口：真机点托盘图标要靠 UI 自动化找图标位置，
/// 先用它量「提到前台 → 8 秒后自己还回桌面层」「显示/隐藏」与「开机自启」到底有没有生效。
#[tauri::command]
fn probe_tray(action: String, app: AppHandle) -> &'static str {
    if std::env::var_os("PERCH_PROBE").is_none() {
        return "探针未开启";
    }
    match action.as_str() {
        "raise" => shell::raise_temporary(&app),
        "toggle" => shell::toggle_visible(&app),
        // 托盘菜单里那一项的真实动作：走与菜单事件完全相同的一条路。
        "autostart" => autostart::toggle(&app),
        other => {
            probe_log::note(format!("[tray] 探针：未知动作 {other}"));
            return "未知动作";
        }
    }
    "已执行"
}

/// 用 PrintWindow 抓一个窗口自身的画面。走窗口自绘，不受「桌面上被别的窗口挡住」影响。
fn shot_of(label: &str, hwnd: *mut c_void) {
    let dir = probe_log::spike_dir().join("shots");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        probe_log::note(format!("[shot:{label}] 建目录失败: {e}"));
        return;
    }
    let out = dir.join(format!("perch_{label}.bmp"));
    let line = unsafe { capture::capture(hwnd, &out) };
    probe_log::note(format!("[shot:{label}] {line}"));
}

/// 抓卡片自己。嵌入后 `FindWindowW` 按标题已经找不到它了，只能用启动时绑定的句柄。
fn shot(label: &str) {
    match desktop_layer::card_hwnd().or_else(find_card) {
        Some(hwnd) => {
            shot_of(label, hwnd);
            let hit = unsafe { desktop_layer::hit_test_line() };
            probe_log::note(format!("[shot:{label}] 命中测试：{hit}"));
        }
        None => probe_log::note(format!("[shot:{label}] 找不到卡片窗口")),
    }
}

fn on_main(app: &AppHandle, f: impl FnOnce() + Send + 'static) {
    if let Err(e) = app.run_on_main_thread(f) {
        probe_log::note(format!("[shot] 主线程调度失败: {e}"));
    }
}

/// 抓屏幕上一块真实合成的像素（不是窗口自绘），用来回答「桌面上到底看得见吗」。
fn screen_shot(label: &str, pad: i32) {
    let dir = probe_log::spike_dir().join("shots");
    let _ = std::fs::create_dir_all(&dir);
    let out = dir.join(format!("screen_{label}.bmp"));
    match unsafe { desktop_layer::card_rect() } {
        Some(r) => {
            let line = unsafe {
                capture::screen(r.x - pad, r.y - pad, r.w + 2 * pad, r.h + 2 * pad, &out)
            };
            probe_log::note(format!("[screen:{label}] {line}"));
        }
        None => probe_log::note(format!("[screen:{label}] 拿不到卡片矩形")),
    }
}

/// `PERCH_SHOTS=1` 时按「普通顶层 → 移到空位 → 嵌入 WorkerW → 还原」逐段抓窗口自绘与屏幕合成，
/// 用来回答 §5.2 的核心风险：WebView2 在 WorkerW 子窗口里到底有没有画出来、桌面上看不看得见。
/// 这条路径下启动时不自动嵌入，否则第一张就不是「普通顶层」了。
fn spawn_shot_sequence(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(2500));
        shot("01_plain");

        // 桌面上有别的窗口压着时，右下角根本看不见，先找一个空位。
        let clear = unsafe {
            desktop_layer::card_rect().and_then(|c| desktop_layer::find_clear_rect(c.w, c.h))
        };
        if let Some(r) = clear {
            let h = handle.clone();
            on_main(&handle, move || {
                if let Some(win) = h.get_webview_window("main") {
                    if let Err(e) = win.set_position(PhysicalPosition::new(r.x, r.y)) {
                        probe_log::note(format!("[shot] 移到空位失败: {e}"));
                    }
                }
                desktop_layer::set_forced_rect(r);
            });
            std::thread::sleep(Duration::from_millis(1200));
            screen_shot("02_plain_screen", 30);
        } else {
            probe_log::note("[shot] 没找到空位，屏幕合成实测只能对着被挡住的桌面做");
        }

        on_main(&handle, || {
            let layer = unsafe { desktop_layer::embed(desktop_layer::DEFAULT_MARGIN) };
            probe_log::note(format!("[shot] 嵌入结果：{}", layer.label()));
        });
        std::thread::sleep(Duration::from_secs(3));
        shot("03_embedded");
        screen_shot("04_embedded_screen", 30);
        // 桌面上有全屏窗口压着时，屏幕抓图证明不了什么；改抓父窗口自己这一层，
        // 看卡片有没有被合成进桌面层。
        match desktop_layer::parent_hwnd() {
            Some(p) => shot_of("07_parent_layer", p),
            None => probe_log::note("[shot] 没有父窗口，抓不了桌面层"),
        }
        // 模拟 Explorer 重启的广播，验证监听线程收得到、并且能在自己的线程上重挂。
        probe_log::note(format!(
            "[shot] 模拟 Explorer 重启：{}",
            desktop_layer::poke_shell_event()
        ));
        std::thread::sleep(Duration::from_secs(2));
        shot("08_after_shell_event");
        on_main(&handle, || unsafe {
            desktop_layer::unembed();
        });
        std::thread::sleep(Duration::from_millis(1500));
        shot("05_restored");
        screen_shot("06_restored_screen", 30);
        probe_log::note("[shot] 抓图序列结束，退出");
        handle.exit(0);
    });
}

#[tauri::command]
fn probe_shot(label: Option<String>) -> String {
    let safe: String = label
        .unwrap_or_else(|| format!("manual_{}", probe_log::stamp()))
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    shot(&safe);
    format!("{safe} 已抓，结论在 spike.log")
}

fn main() {
    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            probe_ping,
            probe_status,
            probe_shot,
            probe_embed,
            probe_unembed,
            probe_cycle,
            probe_shell_event,
            probe_shift,
            probe_quit,
            probe_tray,
            probe_morning,
            commands::view_state,
            commands::manage_state,
            commands::habit_add,
            commands::habit_update,
            commands::habit_goal,
            commands::habit_archive,
            commands::habit_stats,
            commands::habit_remove,
            commands::checkin_set,
            commands::mood_set,
            commands::note_add,
            commands::records_state,
            commands::note_update,
            commands::note_remove,
            commands::note_restore,
            commands::stats_state,
            commands::marker_add,
            commands::marker_remove,
            export::export_preview,
            export::export_save,
            export::export_json,
            export::export_open_folder,
            commands::prefs_get,
            commands::prefs_set,
            autostart::autostart_set,
            autostart::autostart_decline,
            ui_ready,
            card_view::card_set_view,
            card_view::card_target_size,
            card_view::window_set_bounds,
            card_view::window_shift_by,
            card_view::probe_place,
            quick::quick_toggle,
            quick::quick_hide,
            quick::quick_save,
            sync::sync_status,
            sync::sync_now,
            sync::sync_settings,
            sync::sync_configure
        ])
        .setup(|app| {
            // 数据打不开就别开这张卡：半死的界面比一句明确的失败更难查。
            let perch = store::Perch::open()?;
            probe_log::note(format!(
                "[boot] 数据已打开，设备 {}，习惯 {}",
                perch.device_id(),
                perch.data().habits.len()
            ));
            app.manage(store::SharedState::new(perch));
            let prefs = store::Prefs::load(&store::app_root()?);
            let (w, h) = card_view::size_for(boot_slot(prefs.settings()));
            let Some(win) = app.get_webview_window("main") else {
                return Err("找不到 main 窗口".into());
            };
            // 卡片是桌面层的小工具，不是可切换的应用窗口：不进任务栏也不进 Alt+Tab（§5.2）。
            if let Ok(hwnd) = win.hwnd() {
                unsafe { desktop_layer::mark_tool_window(hwnd.0) };
            }
            // 窗口是 visible:false 创建的：先把尺寸与坐标摆好，等界面画完第一帧再露面。
            // 露出来那一刻就是记住的样子，不会先闪一下右下角再跳过去。
            win.set_size(tauri::LogicalSize::new(w as f64, h as f64))?;
            match saved_position(&win, prefs.settings()) {
                Some(pos) => {
                    win.set_position(pos)?;
                    probe_log::note(format!(
                        "[boot] 恢复位置 ({}, {})，折叠档 {}",
                        pos.x,
                        pos.y,
                        boot_slot(prefs.settings())
                    ));
                }
                None => place_bottom_right(&win, desktop_layer::DEFAULT_MARGIN),
            }
            app.manage(store::SharedPrefs::new(prefs));
            // 同步状态（ETag、基线、失败计数）属于本机设置，不进 data.json、不参与同步（§4.4）。
            app.manage(sync::Syncer::load(store::app_root()?));
            // 界面写下的新记录要叫醒同步线程；同步自己写回的合并结果不走这条钩子（store::apply_merged）。
            {
                let handle = app.handle().clone();
                app.state::<store::SharedState>().with(|perch| {
                    perch.set_on_change(std::sync::Arc::new(move || {
                        if let Some(syncer) = handle.try_state::<sync::Syncer>() {
                            syncer.touch();
                        }
                    }));
                    Ok(())
                })?;
            }
            app.manage(MoveSignal::new());
            start_position_saver(app.handle());
            spawn_show_watchdog(app.handle());
            probe_log::note(format!(
                "[boot] spike 启动 pid={} 标题={}",
                std::process::id(),
                win.title().unwrap_or_default()
            ));
            match find_card() {
                Some(hwnd) => {
                    desktop_layer::bind_card(hwnd);
                    desktop_layer::start_watchdog();
                    desktop_layer::start_heartbeat();
                    if std::env::var_os("PERCH_SHOTS").is_some() {
                        probe_log::note("[boot] PERCH_SHOTS 已设：先不嵌入，等三连抓自己走嵌入");
                        spawn_shot_sequence(app.handle());
                    } else if desktop_layer::embed_requested() {
                        let layer = unsafe { desktop_layer::embed(desktop_layer::DEFAULT_MARGIN) };
                        probe_log::note(format!("[boot] 方案 A，初始嵌入结果：{}", layer.label()));
                    } else {
                        unsafe { desktop_layer::settle_bottom() };
                        probe_log::note(
                            "[boot] 方案 B：普通窗口压到应用层最底（§5.2，桌面层被第三方软件占着）",
                        );
                    }
                }
                None => probe_log::note("[boot] 按标题找不到卡片窗口，桌面层测试没跑起来"),
            }
            // 托盘常驻：卡片平时压在桌面层，靠它叫回来（§5.3）。
            shell::build(app.handle());
            // 早晨自动浮出：等界面把第一帧画完再判断，开机第一次露面不该同时改两件事（§2.3）。
            morning::spawn(app.handle());
            // 全局快捷键在自己的线程里收 WM_HOTKEY；注册失败只提示，不影响托盘与卡片里的入口。
            hotkey::spawn(app.handle().clone());
            // 同步的后台线程：不配也不会空转，它只是睡着等设置或手动请求（§4.3）。
            sync::spawn(app.handle());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("构建 Perch spike 失败");

    app.run(|app, event| match event {
        tauri::RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::Moved(_),
            ..
        } if label == "main" => app.state::<MoveSignal>().bump(),
        // 窗口重新拿到焦点：用户回来了，尽快同步一次；节流仍由同步线程按 §4.3 管。
        tauri::RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::Focused(true),
            ..
        } if label == "main" => {
            if let Some(syncer) = app.try_state::<sync::Syncer>() {
                syncer.soon();
            }
        }
        tauri::RunEvent::Exit => {
            flush_position(app);
            probe_log::note("[exit] 退出前还原窗口层级");
            unsafe {
                desktop_layer::unembed();
            }
        }
        _ => {}
    });
}
