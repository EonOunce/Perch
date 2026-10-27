//! 早晨自动浮出（DESIGN §2.3、§5.3）。
//!
//! 开机后卡片自己提一下层级，让她看见「昨天还有东西没确认」。三条硬约束：
//! 每天最多一次、只在真有待确认项时才浮、只提层级不抢键盘焦点。
//! 永久置顶是不变项 #5 明确不做的，所以这里复用托盘那套「短时提到前台」。

use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

use crate::probe_log;
use crate::store::{SharedPrefs, SharedState};

/// 启动后等多久再浮。窗口刚露面就提层会像闪了一下，等它把第一帧画完、位置摆好。
const SETTLE: Duration = Duration::from_millis(1200);

/// 后台等一拍再判断。窗口只提层级、不抢焦点，所以整段可以在这个线程上走完
/// （`shell::raise_quiet` 自己会把动窗口的部分排回主线程）。
pub fn spawn(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(SETTLE);
        decide(&handle);
    });
}

/// 今天该不该浮一次。三个条件缺一个就什么都不做，也不记「今天浮过了」。
/// `pub(crate)` 只为了让探针能立刻走一遍同一条判断，不复制一份逻辑。
pub(crate) fn decide(app: &AppHandle) {
    let today = today();
    let Some(prefs) = app.try_state::<SharedPrefs>() else {
        probe_log::note("[morning] 设置还没挂上，这次不浮");
        return;
    };
    let due = prefs
        .with(|prefs| {
            let settings = prefs.settings();
            Ok(settings.morning_raise
                && settings.morning_raise_date.as_deref() != Some(today.as_str()))
        })
        .unwrap_or(false);
    if !due {
        return;
    }
    let Some(state) = app.try_state::<SharedState>() else {
        return;
    };
    // 读不出数据（文件坏了、只读）就当没有待确认：宁可不浮，也不要在卡片已经报错时再推一下。
    let Ok(count) = state.with(|perch| Ok(crate::commands::awaiting_count(perch))) else {
        return;
    };
    // 没有要确认的东西就不浮——§2.3 那句「不做无意义打扰」是这条存在的理由。
    if count == 0 {
        probe_log::note("[morning] 没有待确认项，不浮");
        return;
    }
    // 先记账再动窗口：万一提层中途出岔子，也不该在同一天里再拿一次打扰去补。
    if let Err(e) = prefs
        .with(|prefs| prefs.update(|settings| settings.morning_raise_date = Some(today.clone())))
    {
        probe_log::note(format!("[morning] 记不下今天已经浮过：{e}"));
        return;
    }
    crate::shell::raise_quiet(app);
    // 数的是「项」（一个习惯的一晚算一项），和折叠日期栏上那个计数同一个口径。
    let _ = app.emit("perch:hint", format!("早安。昨天还有 {count} 项待确认。"));
    probe_log::note(format!("[morning] {today} 浮出一次，待确认 {count} 项"));
}

/// 开关现在是什么状态。和自启一样只读设置，界面不另存一份。
pub fn is_on(app: &AppHandle) -> bool {
    app.try_state::<SharedPrefs>()
        .and_then(|prefs| prefs.with(|prefs| Ok(prefs.settings().morning_raise)).ok())
        .unwrap_or(false)
}

/// 浮出与否的开关（托盘菜单那一项）。只改设置，不顺手补一次今天的浮出，
/// 也不动「今天浮过没有」——关了又开回来，今天不该连着浮两次。
pub fn toggle(app: &AppHandle) {
    let Some(prefs) = app.try_state::<SharedPrefs>() else {
        probe_log::note("[morning] 设置还没挂上，改不了");
        return;
    };
    let flipped = prefs.with(|prefs| {
        let next = !prefs.settings().morning_raise;
        prefs.update(|settings| settings.morning_raise = next)?;
        Ok(next)
    });
    match flipped {
        Ok(next) => {
            crate::shell::refresh_menu(app);
            probe_log::note(format!("[morning] 早晨浮出改成 {next}"));
        }
        Err(e) => {
            probe_log::note(format!("[morning] 改不动：{e}"));
            let _ = app.emit("perch:hint", format!("早晨浮出没改成：{e}"));
        }
    }
}

/// 现在算「今天」是哪一天。日界 04:00 的口径在 perch-core 的 `Now::date`，这里只是问一次。
fn today() -> String {
    crate::clock::now().date()
}

#[cfg(test)]
mod tests {
    use crate::store::{Prefs, Settings, SharedPrefs};
    use std::fs;
    use std::path::PathBuf;

    fn temp_prefs(tag: &str) -> (SharedPrefs, PathBuf) {
        let dir = std::env::temp_dir().join(format!("perch-morning-{tag}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        (SharedPrefs::new(Prefs::load(&dir)), dir)
    }

    fn settings(prefs: &SharedPrefs) -> Settings {
        prefs.with(|p| Ok(p.settings().clone())).unwrap()
    }

    /// 默认是开着，但「今天浮过没有」必须还没有值：第一次开机就该能浮一次。
    #[test]
    fn the_morning_raise_starts_on_and_unfired() {
        let (prefs, dir) = temp_prefs("default");
        let saved = settings(&prefs);
        assert!(
            saved.morning_raise,
            "§2.3 把它做成可关的选项，不是默认关的功能"
        );
        assert_eq!(saved.morning_raise_date, None);
        let _ = fs::remove_dir_all(dir);
    }

    /// 「每天最多一次」靠的是落盘的日期，不是内存里的标志：重启不能又浮一次。
    #[test]
    fn the_fired_date_survives_a_reopen() {
        let (prefs, dir) = temp_prefs("fired");
        prefs
            .with(|prefs| {
                prefs
                    .update(|settings| settings.morning_raise_date = Some("2026-10-28".into()))
                    .unwrap();
                Ok(())
            })
            .unwrap();
        let again = settings(&SharedPrefs::new(Prefs::load(&dir)));
        assert_eq!(again.morning_raise_date.as_deref(), Some("2026-10-28"));
        assert!(again.morning_raise);
        let _ = fs::remove_dir_all(dir);
    }
}
