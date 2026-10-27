//! 开机自启（DESIGN §5.3、§12 #6）。
//!
//! 只写当前用户这一项 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`：
//! 不碰机器级启动项、不注册服务、不用任务计划程序。默认关，且只有她自己点过才写。
//!
//! 顺序是先改系统再改设置：设置说「开着」的时候，注册表里就必须真的开着。

use std::path::Path;

use crate::probe_log;
use crate::store::SharedPrefs;
use tauri::{AppHandle, Emitter, Manager};
// windows-sys 0.59 里 `PCWSTR` 只是 `*const u16` 的别名，所以 UTF-16 缓冲直接递指针。
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_SET_VALUE, REG_SZ,
};

pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const VALUE_NAME: &str = "Perch";

/// 写进 Run 键的那一行。整条路径要包在引号里：装在 `Program Files` 这类带空格的
/// 目录下时，不包引号会在第一个空格处被截断，开机就什么都起不来。
pub fn run_value(exe: &Path) -> Result<String, String> {
    let text = exe
        .to_str()
        .ok_or_else(|| "程序路径读不成文本，写不了开机启动项".to_string())?;
    Ok(format!("\"{text}\""))
}

/// 开机自启的开关。写系统启动项这一步在设置之前：界面说「开着」的时候，
/// 注册表里就必须真的开着，反过来则会出现一个点了却没生效的勾选框。
#[tauri::command]
pub fn autostart_set(enabled: bool, app: AppHandle) -> Result<(), String> {
    apply(&app, enabled)
}

/// 首次启动那次问答的另一半：回答「先不用」只结束问题，不写任何系统改动（§12 #6）。
#[tauri::command]
pub fn autostart_decline(app: AppHandle) -> Result<(), String> {
    mark_asked(&app.state::<SharedPrefs>())
}

/// 按她的选择改系统，改成了才改设置。返回错误时设置保持原样。
/// 只要是明确回答过的，「问过了」就一起钉上——§12 #6 说的只问一次不能只靠界面记着。
pub fn commit(prefs: &SharedPrefs, enabled: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("拿不到自己在哪：{e}"))?;
    if enabled {
        write_line(&run_value(&exe)?)?;
    } else {
        drop_line()?;
    }
    match answered(prefs, enabled) {
        Ok(()) => Ok(()),
        Err(e) => {
            probe_log::note(format!(
                "[autostart] 启动项已改成 {enabled}，但设置没记住：{e}"
            ));
            Err(e)
        }
    }
}

/// 开与关都要把「问过了」钉成 true：§12 #6 说的是只问一次，问过就不再追问。
fn answered(prefs: &SharedPrefs, enabled: bool) -> Result<(), String> {
    prefs.with(|prefs| {
        prefs.update(|settings| {
            settings.autostart = enabled;
            settings.autostart_asked = true;
        })
    })
}

/// 首次启动那一次选了「先不用」：只把问题关掉，不碰注册表也不改开关值。
/// 走 `commit(false)` 会顺手删掉可能已经存在的启动项，那不是她这次要的事。
pub fn mark_asked(prefs: &SharedPrefs) -> Result<(), String> {
    prefs.with(|prefs| prefs.update(|settings| settings.autostart_asked = true))
}

/// 现在算不算开着。读设置不读注册表：设置是 `commit` 改成功之后才跟着写的，
/// 两者只在「她自己在注册表里手动动过」时才可能不一致，而那已经不是这个开关管的事。
pub fn is_on(app: &AppHandle) -> bool {
    app.try_state::<SharedPrefs>()
        .and_then(|prefs| prefs.with(|prefs| Ok(prefs.settings().autostart)).ok())
        .unwrap_or(false)
}

/// 界面与托盘都走这一条：先落到系统，再刷新托盘那一项的勾选。
pub fn apply(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let result = commit(&app.state::<SharedPrefs>(), enabled);
    crate::shell::refresh_menu(app);
    result
}

/// 托盘那一项点一下就是「翻个面」：读到的当前值来自设置，不是界面自己记的。
/// 没改成要说出来——一个点了却没反应的勾选框比一句失败原因更难查。
pub fn toggle(app: &AppHandle) {
    let next = !is_on(app);
    if let Err(e) = apply(app, next) {
        probe_log::note(format!("[autostart] 托盘切换失败：{e}"));
        let _ = app.emit("perch:hint", format!("开机自启没改成：{e}"));
    }
}

fn write_line(value: &str) -> Result<(), String> {
    let key = Key::open(KEY_SET_VALUE)?;
    unsafe {
        let name = wide(VALUE_NAME);
        let data = wide(value);
        let code = RegSetValueExW(
            key.0,
            name.as_ptr(),
            0,
            REG_SZ,
            data.as_ptr() as *const u8,
            (data.len() * 2) as u32,
        );
        if code == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(format!("写开机启动项没成功（注册表返回 {code}）"))
        }
    }
}

/// 关：删掉这一行。这一项本来就不存在算已经关好了。
fn drop_line() -> Result<(), String> {
    let Ok(key) = Key::open(KEY_SET_VALUE) else {
        return Ok(());
    };
    unsafe {
        let name = wide(VALUE_NAME);
        let code = RegDeleteValueW(key.0, name.as_ptr());
        if code == ERROR_SUCCESS || code == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(format!("删开机启动项没成功（注册表返回 {code}）"))
        }
    }
}

/// 句柄一定要关；用 Drop 包住，错误分支就不会漏。
struct Key(HKEY);

impl Key {
    fn open(sam: u32) -> Result<Key, String> {
        unsafe {
            let name = wide(RUN_KEY);
            let mut out: HKEY = std::ptr::null_mut();
            let code = RegOpenKeyExW(HKEY_CURRENT_USER, name.as_ptr(), 0, sam, &mut out);
            if code == ERROR_SUCCESS {
                Ok(Key(out))
            } else {
                Err(format!("打不开注册表启动项（返回 {code}）"))
            }
        }
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.0) };
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::{answered, mark_asked, run_value};
    use crate::store::{Prefs, SharedPrefs};
    use std::fs;
    use std::path::{Path, PathBuf};

    fn temp_prefs(tag: &str) -> (SharedPrefs, PathBuf) {
        let dir = std::env::temp_dir().join(format!("perch-autostart-{tag}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        (SharedPrefs::new(Prefs::load(&dir)), dir)
    }

    fn settings(prefs: &SharedPrefs) -> crate::store::Settings {
        prefs.with(|p| Ok(p.settings().clone())).unwrap()
    }

    /// 带空格的目录是最常见的死法：不整体引号化，开机时只会去执行 `C:\Program`。
    #[test]
    fn the_run_line_wraps_the_whole_path_in_quotes() {
        let line = run_value(Path::new(r"C:\Program Files\Perch\perch.exe")).unwrap();
        assert_eq!(line, r#""C:\Program Files\Perch\perch.exe""#);
    }

    /// 开与关都要重读一遍才知道记住了没有：设置是下次开机画勾选框的唯一依据。
    #[test]
    fn answering_either_way_ends_the_question() {
        for answer in [true, false] {
            let (prefs, dir) = temp_prefs(&format!("asked-{answer}"));
            answered(&prefs, answer).unwrap();
            let again = SharedPrefs::new(Prefs::load(&dir));
            let saved = settings(&again);
            assert_eq!(saved.autostart, answer, "开关值没落盘");
            assert!(saved.autostart_asked, "问过之后不该再问");
            let _ = fs::remove_dir_all(dir);
        }
    }

    /// 选「先不用」不写注册表，也不许顺手把已有的启动项删掉：
    /// 它只结束这一次问答，`autostart` 留在原值。
    #[test]
    fn declining_only_ends_the_question() {
        let (prefs, dir) = temp_prefs("decline");
        mark_asked(&prefs).unwrap();
        let saved = settings(&SharedPrefs::new(Prefs::load(&dir)));
        assert!(!saved.autostart, "没点过开启就不该说自己开着");
        assert!(saved.autostart_asked);
        let _ = fs::remove_dir_all(dir);
    }

    /// 默认是关，而且没问过：不静默写系统启动项（§12 #6）。
    #[test]
    fn autostart_starts_off_and_unasked() {
        let (prefs, dir) = temp_prefs("default");
        assert!(!settings(&prefs).autostart);
        assert!(!settings(&prefs).autostart_asked);
        let _ = fs::remove_dir_all(dir);
    }
}
