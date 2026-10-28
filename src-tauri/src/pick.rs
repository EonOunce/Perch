//! 选一个文件：Windows 自带的打开文件框（DESIGN §4.7 的恢复入口）。
//!
//! 不引第三方对话框依赖——`comdlg32` 的 `GetOpenFileNameW` 已经在现有的 windows-sys 里，
//! 而这里只需要「选一个存在的 .json」。恢复要读的文件来自用户，所以路径只从这个框里来，
//! 前端拿不到、也传不进任意路径。

use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_DONTADDTORECENT, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY,
    OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};

/// 打开「选择 JSON」对话框；取消返回 `None`。
///
/// `owner` 是卡片的 HWND：没有属主时对话框可能出现在卡片后面（卡片平时在桌面层最底）。
/// 这个函数会自己跑消息循环，调用方应放在工作线程里，别卡住界面线程。
pub fn open_json(owner: Option<isize>, start: Option<&Path>) -> Option<PathBuf> {
    let mut buffer = vec![0u16; 32 * 1024];
    let filter: Vec<u16> = "栖迟数据 (*.json)\0*.json\0所有文件 (*.*)\0*.*\0\0"
        .encode_utf16()
        .collect();
    let title: Vec<u16> = "选择要恢复的快照\0".encode_utf16().collect();
    let initial: Vec<u16> = start
        .map(|dir| {
            dir.to_string_lossy()
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect()
        })
        .unwrap_or_default();

    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = owner
        .map(|handle| handle as HWND)
        .unwrap_or(std::ptr::null_mut());
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = buffer.as_mut_ptr();
    ofn.nMaxFile = buffer.len() as u32;
    ofn.lpstrTitle = title.as_ptr();
    if !initial.is_empty() {
        ofn.lpstrInitialDir = initial.as_ptr();
    }
    ofn.Flags = OFN_EXPLORER
        | OFN_FILEMUSTEXIST
        | OFN_PATHMUSTEXIST
        | OFN_NOCHANGEDIR
        | OFN_HIDEREADONLY
        | OFN_DONTADDTORECENT;

    if unsafe { GetOpenFileNameW(&mut ofn) } == 0 {
        return None;
    }
    let len = buffer.iter().position(|&unit| unit == 0).unwrap_or(0);
    if len == 0 {
        return None;
    }
    String::from_utf16(&buffer[..len]).ok().map(PathBuf::from)
}
