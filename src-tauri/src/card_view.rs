//! 卡片的尺寸档（DESIGN §2.1）：今天 360×480，记录与统计 520×680，
//! 折叠两档 360×169（日期栏 + 竖向长条）与 360×46（长条整体收起）。
//!
//! 前端只说「现在看哪个标签／折叠到哪一档」，窗口几何全部由这一层改。尺寸表如果在 JS 里再写一份，
//! 换标签时就会出现「标签是记录、窗口还是今天的尺寸」这类对不上的状态。

use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewWindow};

/// 今天视图与另外两个标签的窗口尺寸（CSS 逻辑像素，WebView2 按系统缩放放大）。
const TODAY: (u32, u32) = (360, 480);
const WIDE: (u32, u32) = (520, 680);
/// 折叠态：日期栏下面挂着竖向长条（§2.8）。
const STRIP: (u32, u32) = (360, 169);
/// 长条收起后只剩横向日期栏。
const BAR: (u32, u32) = (360, 46);

pub fn size_for(view: &str) -> (u32, u32) {
    match view {
        "today" => TODAY,
        "collapsed" => STRIP,
        "bar" => BAR,
        _ => WIDE,
    }
}

/// 切标签时顺带改窗口尺寸，右下角保持不动。
#[tauri::command]
pub fn card_set_view(view: String, app: AppHandle) -> Result<(), String> {
    let win = app
        .get_webview_window("main")
        .ok_or_else(|| "找不到 main 窗口".to_string())?;
    resize(&win, size_for(&view))
}

/// 只在 `PERCH_PROBE=1` 时可用：把窗口挪到给定物理坐标。
/// 用途是把卡片挪到一块干净的桌面上，用真鼠标量 4px 拖动阈值——本机右下角整个被 IDE 窗口压着。
/// 正式实现里位置由本地设置恢复，这个入口随探针一起退场。
#[tauri::command]
pub fn probe_place(x: i32, y: i32, app: AppHandle) -> Result<(), String> {
    if std::env::var_os("PERCH_PROBE").is_none() {
        return Err("探针未开启".to_string());
    }
    let win = app
        .get_webview_window("main")
        .ok_or_else(|| "找不到 main 窗口".to_string())?;
    win.set_position(PhysicalPosition::new(x, y))
        .map_err(|e| e.to_string())
}

fn resize(win: &WebviewWindow, (w, h): (u32, u32)) -> Result<(), String> {
    let scale = win.scale_factor().map_err(|e| e.to_string())?;
    let old_outer = win.outer_position().map_err(|e| e.to_string())?;
    let old_inner = win.inner_position().map_err(|e| e.to_string())?;
    let old_size = win.inner_size().map_err(|e| e.to_string())?;
    let next = PhysicalSize::new(
        (w as f64 * scale).round() as u32,
        (h as f64 * scale).round() as u32,
    );
    if next == old_size {
        return Ok(());
    }
    win.set_size(next).map_err(|e| e.to_string())?;
    // 卡片平时贴在屏幕右下角，所以按右下角对齐地往上、往左长；再夹回工作区，
    // 免得被拖到边角时换个大尺寸就有一半看不见。
    // `set_size` 改的是内容区，锚点就必须同样用内容区算，最后折算回 `set_position`
    // 要的窗口外框坐标：内外两套数字混着用，卡片每换一次标签就往右下方挪掉一圈边框宽。
    let border = (old_inner.x - old_outer.x, old_inner.y - old_outer.y);
    let x = old_inner.x + old_size.width as i32 - next.width as i32 - border.0;
    let y = old_inner.y + old_size.height as i32 - next.height as i32 - border.1;
    let (x, y) = match win.current_monitor().ok().flatten() {
        Some(monitor) => {
            let area = monitor.work_area();
            let max_x =
                area.position.x + area.size.width as i32 - (next.width as i32 + 2 * border.0);
            let max_y =
                area.position.y + area.size.height as i32 - (next.height as i32 + 2 * border.1);
            (
                x.clamp(area.position.x, max_x.max(area.position.x)),
                y.clamp(area.position.y, max_y.max(area.position.y)),
            )
        }
        None => (x, y),
    };
    win.set_position(PhysicalPosition::new(x, y))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::size_for;

    /// 尺寸表是 §2.1 那张表的抄本，钉住免得改一处就和文档对不上。
    #[test]
    fn the_four_size_slots_match_the_design_table() {
        assert_eq!(size_for("today"), (360, 480));
        assert_eq!(size_for("records"), (520, 680));
        assert_eq!(size_for("stats"), (520, 680));
        assert_eq!(size_for("collapsed"), (360, 169));
        assert_eq!(size_for("bar"), (360, 46));
    }
}
