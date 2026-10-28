//! 探针日志：追加写入 `%LOCALAPPDATA%\Perch\spike.log`，同时打到 stdout。
//! 只在 spike 阶段用，正式实现换正经日志。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

static PATH: OnceLock<PathBuf> = OnceLock::new();

fn log_path() -> PathBuf {
    PATH.get_or_init(|| {
        let root = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let dir = root.join("Perch");
        let _ = std::fs::create_dir_all(&dir);
        dir.join("spike.log")
    })
    .clone()
}

/// spike 的产物目录（截图、临时数据）跟日志放一起，都在 %LOCALAPPDATA%\Perch 下。
pub fn spike_dir() -> PathBuf {
    log_path()
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(PathBuf::new)
}

pub fn note(args: impl std::fmt::Display) {
    let line = format!("{}\n", args);
    print!("{line}");
    let _ = std::io::stdout().flush();
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// 没有控制台以后，panic 的默认输出没人看得见；至少让它落进 spike.log。
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        note(format!("[panic] {info}"));
        previous(info);
    }));
}

pub fn stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}
