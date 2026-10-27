#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod clock;
mod commands;
mod desktop_layer;
mod probe_log;
mod store;

use std::ffi::c_void;
use std::time::Duration;

use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewWindow};
use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW;

const PROBE_TITLE: &str = "PerchLayerProbe";

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
    let x = area.position.x + area.size.width as i32 - size.width as i32 - margin;
    let y = area.position.y + area.size.height as i32 - size.height as i32 - margin;
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
        Ok(()) => format!("Tauri set_position {before:?} → {after:?}；实际 {:?}", window.outer_position().map(|p| (p.x, p.y))),
        Err(e) => format!("移动失败: {e}"),
    }
}

#[tauri::command]
fn probe_quit(app: AppHandle) {
    app.exit(0);
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
            let line = unsafe { capture::screen(r.x - pad, r.y - pad, r.w + 2 * pad, r.h + 2 * pad, &out) };
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
        let clear = unsafe { desktop_layer::card_rect().and_then(|c| desktop_layer::find_clear_rect(c.w, c.h)) };
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
            commands::view_state,
            commands::habit_add,
            commands::checkin_set,
            commands::mood_set,
            commands::note_add
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
            let Some(win) = app.get_webview_window("main") else {
                return Err("找不到 main 窗口".into());
            };
            place_bottom_right(&win, desktop_layer::DEFAULT_MARGIN);
            win.show()?;
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
                    } else {
                        let layer = unsafe { desktop_layer::embed(desktop_layer::DEFAULT_MARGIN) };
                        probe_log::note(format!("[boot] 初始嵌入结果：{}", layer.label()));
                    }
                }
                None => probe_log::note("[boot] 按标题找不到卡片窗口，桌面层测试没跑起来"),
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("构建 Perch spike 失败");

    app.run(|_app, event| {
        if let tauri::RunEvent::Exit = event {
            probe_log::note("[exit] 退出前还原窗口层级");
            unsafe {
                desktop_layer::unembed();
            }
        }
    });
}
