//! 导出文件的落点与命名（DESIGN §2.5、§4.6、§6.5）。
//!
//! 内容怎么排是 `perch-core::export` 的事；这里只管三件系统相关的事：找到用户看得见的
//! 文件夹、起一个不会互相覆盖的名字、一次写完。perch-core 因此永远不用知道
//! `%USERPROFILE%` 长什么样（§5.4）。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::clock;
use crate::store::SharedState;

/// 用户能自己找到的文件夹：`文档\栖迟导出`。
///
/// 没有文档目录（少见，但组策略或精简系统会有）时退回 `%LOCALAPPDATA%\Perch\exports`——
/// 导出不能因为文件夹不在就失败，界面上会把实际路径写出来。
pub fn folder() -> Result<PathBuf, String> {
    let docs = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|home| home.join("Documents"))
        .filter(|path| path.is_dir());
    let dir = match docs {
        Some(docs) => docs.join("栖迟导出"),
        None => crate::store::app_root()?.join("exports"),
    };
    fs::create_dir_all(&dir).map_err(|err| format!("建不了导出文件夹 {}：{err}", dir.display()))?;
    Ok(dir)
}

/// 文件名由应用生成，但仍然挡掉分隔符与 `..`：路径只从这一处进，将来改参数也带不进用户文本。
fn safe_name(name: &str) -> Result<&str, String> {
    let bad = name.trim().is_empty()
        || name
            .chars()
            .any(|c| matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        || name.contains("..")
        || name.starts_with('.');
    if bad {
        Err("导出文件名不合法。".to_string())
    } else {
        Ok(name)
    }
}

/// 不覆盖已有文件：重名就加 `-2`、`-3`。用 `create_new` 占位，抢占失败就换下一个名字。
fn create_unique(folder: &Path, name: &str) -> Result<(PathBuf, fs::File), String> {
    let name = safe_name(name)?;
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("栖迟导出");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    for index in 1..1000 {
        let candidate = if index == 1 {
            name.to_string()
        } else if ext.is_empty() {
            format!("{stem}-{index}")
        } else {
            format!("{stem}-{index}.{ext}")
        };
        let full = folder.join(&candidate);
        match OpenOptions::new().write(true).create_new(true).open(&full) {
            Ok(file) => return Ok((full, file)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(format!("写不了 {}：{err}", full.display())),
        }
    }
    Err("导出文件夹里的同名文件太多了，先整理一下。".to_string())
}

/// 一次写完；写一半失败就把半截文件删掉，不留下一个看起来像导出成功的空壳。
pub fn save(folder: &Path, name: &str, text: &str) -> Result<PathBuf, String> {
    let (path, mut file) = create_unique(folder, name)?;
    if let Err(err) = file
        .write_all(text.as_bytes())
        .and_then(|_| file.sync_all())
    {
        drop(file);
        let _ = fs::remove_file(&path);
        return Err(format!("写 {} 失败：{err}", path.display()));
    }
    Ok(path)
}

/// 用资源管理器打开导出文件夹。打不开不算导出失败——文件已经在盘上了，路径也告诉界面了。
pub fn open(folder: &Path) -> Result<(), String> {
    std::process::Command::new("explorer.exe")
        .arg(folder)
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("打不开文件夹：{err}"))
}

/// 文件名里的时间戳：`2026-10-28T14:05:00.000+08:00` → `20261028-1405`。
/// 读不出来就留空，不编一个看起来像今天的日期。
pub fn stamp(stamp: &str) -> String {
    let date = stamp.get(0..10).unwrap_or("").replace('-', "");
    let time = stamp.get(11..16).unwrap_or("").replace(':', "");
    if date.is_empty() {
        String::new()
    } else if time.is_empty() {
        date
    } else {
        format!("{date}-{time}")
    }
}

pub fn file_name(title: &str, stamp_text: &str, ext: &str) -> String {
    let stamp = stamp(stamp_text);
    if stamp.is_empty() {
        format!("{title}.{ext}")
    } else {
        format!("{title}-{stamp}.{ext}")
    }
}

fn now_stamp() -> String {
    clock::now().stamp()
}

/// 「导出时间」那一行：`2026-10-28 14:05`。用日历日期，不用 04:00 的业务日——
/// 导出的是文件，看的人按钟表理解。
fn now_label() -> String {
    let stamp = now_stamp();
    let date = stamp.get(0..10).unwrap_or("");
    let time = stamp.get(11..16).unwrap_or("");
    if date.is_empty() {
        String::new()
    } else {
        format!("{date} {time}").trim_end().to_string()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPreview {
    pub text: String,
    /// 建议的文件名；保存时原样带回来，用户改不了，也不会从别的路径进来。
    pub name: String,
    pub folder: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSaved {
    pub path: String,
    pub folder: String,
}

/// 记一条的导出文本。记录与心情都来自当前数据文件，不重新解释任何口径。
#[tauri::command]
pub fn export_preview(
    kind: String,
    state: tauri::State<'_, SharedState>,
) -> Result<ExportPreview, String> {
    let folder = folder()?;
    let data = state.with(|perch| Ok(perch.data().clone()))?;
    let stamp = now_stamp();
    let (text, name) = match kind.as_str() {
        "notes" => (
            perch_core::export::notes_markdown(&data, &now_label()),
            file_name("栖迟记录", &stamp, "md"),
        ),
        "review" => {
            let now = clock::now();
            let review = perch_core::stats::review(&data, &now.date(), now.offset_secs);
            let name = if review.week_start.is_empty() {
                file_name("栖迟周回顾", &stamp, "md")
            } else {
                format!("栖迟周回顾-{}.md", review.week_start)
            };
            (perch_core::export::review_markdown(&review), name)
        }
        _ => return Err("不认识的导出类型。".to_string()),
    };
    Ok(ExportPreview {
        text,
        name,
        folder: folder.display().to_string(),
    })
}

/// 保存界面里（可能已经改过字的）那份文本。文件名只收应用给的那个建议名。
#[tauri::command]
pub fn export_save(kind: String, name: String, text: String) -> Result<ExportSaved, String> {
    if !matches!(kind.as_str(), "notes" | "review") {
        return Err("不认识的导出类型。".to_string());
    }
    let folder = folder()?;
    let path = save(&folder, &name, &text)?;
    Ok(ExportSaved {
        path: path.display().to_string(),
        folder: folder.display().to_string(),
    })
}

/// 全部数据的 JSON 快照：恢复与转手的最后一道保险（§4.6）。
/// 与 Markdown 分开一个命令：它不经过编辑，写的就是 `data.json` 里那份。
#[tauri::command]
pub fn export_json(state: tauri::State<'_, SharedState>) -> Result<ExportSaved, String> {
    let folder = folder()?;
    let text = state.with(|perch| {
        serde_json::to_string_pretty(perch.data()).map_err(|err| format!("读不出数据：{err}"))
    })?;
    let path = save(
        &folder,
        &file_name("栖迟全部数据", &now_stamp(), "json"),
        &(text + "\n"),
    )?;
    Ok(ExportSaved {
        path: path.display().to_string(),
        folder: folder.display().to_string(),
    })
}

#[tauri::command]
pub fn export_open_folder() -> Result<String, String> {
    let folder = folder()?;
    open(&folder)?;
    Ok(folder.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(tag: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("perch-export-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn the_stamp_keeps_seconds_out_but_keeps_the_day() {
        assert_eq!(stamp("2026-10-28T14:05:09.123+08:00"), "20261028-1405");
        assert_eq!(stamp("坏的"), "");
        assert_eq!(file_name("栖迟记录", "坏的", "md"), "栖迟记录.md");
    }

    #[test]
    fn a_second_export_with_the_same_name_does_not_overwrite_the_first() {
        let dir = temp_dir("unique");
        let first = save(&dir, "栖迟记录-20261028-1405.md", "第一份").unwrap();
        let second = save(&dir, "栖迟记录-20261028-1405.md", "第二份").unwrap();
        assert_ne!(first, second);
        assert_eq!(fs::read_to_string(&first).unwrap(), "第一份");
        assert_eq!(fs::read_to_string(&second).unwrap(), "第二份");
        assert_eq!(
            second.file_name().unwrap().to_str().unwrap(),
            "栖迟记录-20261028-1405-2.md"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn names_with_path_separators_or_dot_dot_are_refused() {
        for bad in [
            "../导出.md",
            "子目录/导出.md",
            "子目录\\导出.md",
            ".隐藏.md",
            "",
        ] {
            assert!(safe_name(bad).is_err(), "{bad:?} 不该被当成文件名");
        }
        assert!(safe_name("栖迟全部数据-20261028-1405.json").is_ok());
    }

    #[test]
    fn the_saved_file_is_utf8_without_a_bom_or_extra_blank_lines() {
        let dir = temp_dir("utf8");
        let path = save(&dir, "栖迟周回顾-2026-10-19.md", "# 回顾\n\n- 一条事实\n").unwrap();
        let bytes = fs::read(&path).unwrap();
        assert!(!bytes.starts_with(&[0xEF, 0xBB, 0xBF]), "不写 BOM");
        assert_eq!(
            std::str::from_utf8(&bytes).unwrap(),
            "# 回顾\n\n- 一条事实\n"
        );
        let _ = fs::remove_dir_all(dir);
    }
}
