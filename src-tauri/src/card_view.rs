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
/// 回一个「尺寸到底变没变」：前端要等网页跟着重排完才能量锚点，尺寸没变就没必要等。
#[tauri::command]
pub fn card_set_view(view: String, app: AppHandle) -> Result<bool, String> {
    let win = app
        .get_webview_window("main")
        .ok_or_else(|| "找不到 main 窗口".to_string())?;
    resize(&win, size_for(&view))
}

/// 尺寸表的只读入口：换档动画要知道目标档有多大。
/// 表仍然只有 Rust 这一份，前端是来读一个数，不是抄一张表。
#[tauri::command]
pub fn card_target_size(view: String) -> (u32, u32) {
    size_for(&view)
}

/// 换档动画的一帧：尺寸与位置在同一个命令里一次改完。
/// 拆成两次调用会有一帧停在旧位置上，卡片看着就是先跳一下再滑——正是要避免的手感。
/// `w`/`h` 是内容区的 CSS 逻辑像素，`x`/`y` 是窗口外框的物理像素；
/// 位置夹回工作区：空间不够时宁可让锚点偏一点，也不把卡片推出屏幕（§8，用户拍板「屏幕优先」）。
#[tauri::command]
pub fn window_set_bounds(x: i32, y: i32, w: u32, h: u32, app: AppHandle) -> Result<(), String> {
    let win = app
        .get_webview_window("main")
        .ok_or_else(|| "找不到 main 窗口".to_string())?;
    let scale = win.scale_factor().map_err(|e| e.to_string())?;
    let inner = PhysicalSize::new(
        (w as f64 * scale).round() as u32,
        (h as f64 * scale).round() as u32,
    );
    win.set_size(inner).map_err(|e| e.to_string())?;
    // 内外坐标系的差就是那圈看不见的调整边框，每帧现读：拿开机时的值会在换 DPI 后错位。
    let inner_pos = win.inner_position().map_err(|e| e.to_string())?;
    let outer_pos = win.outer_position().map_err(|e| e.to_string())?;
    let border = (inner_pos.x - outer_pos.x, inner_pos.y - outer_pos.y);
    let (x, y) = match win.current_monitor().ok().flatten() {
        Some(monitor) => {
            let area = monitor.work_area();
            clamp_into(
                crate::place::Rect {
                    x: area.position.x,
                    y: area.position.y,
                    width: area.size.width,
                    height: area.size.height,
                },
                (
                    inner.width as i32 + 2 * border.0,
                    inner.height as i32 + 2 * border.1,
                ),
                (x, y),
            )
        }
        None => (x, y),
    };
    win.set_position(PhysicalPosition::new(x, y))
        .map_err(|e| e.to_string())
}

/// `resize` 固定按右下角长，所以点上面的标签或折叠按钮时，被点的那个控件会被窗口挪走。
/// 前端量出它移动了多少（CSS 像素），这里把整个窗口平移回去，鼠标就还压在刚点的控件上（§8）。
#[tauri::command]
pub fn window_shift_by(dx: i32, dy: i32, app: AppHandle) -> Result<(), String> {
    let win = app
        .get_webview_window("main")
        .ok_or_else(|| "找不到 main 窗口".to_string())?;
    let outer = win.outer_position().map_err(|e| e.to_string())?;
    let size = win.outer_size().map_err(|e| e.to_string())?;
    let target = (outer.x + dx, outer.y + dy);
    let (x, y) = match win.current_monitor().ok().flatten() {
        // 补偿位移不许把卡片推出屏幕：夹回工作区，宁可鼠标偏一点也别看不见卡片。
        Some(monitor) => {
            let area = monitor.work_area();
            clamp_into(
                crate::place::Rect {
                    x: area.position.x,
                    y: area.position.y,
                    width: area.size.width,
                    height: area.size.height,
                },
                (size.width as i32, size.height as i32),
                target,
            )
        }
        None => target,
    };
    win.set_position(PhysicalPosition::new(x, y))
        .map_err(|e| e.to_string())
}

/// 把外框矩形夹回工作区；窗口比工作区还大时贴着左上角，不留一半在屏外。
fn clamp_into(area: crate::place::Rect, (w, h): (i32, i32), (x, y): (i32, i32)) -> (i32, i32) {
    let max_x = (area.x + area.width as i32 - w).max(area.x);
    let max_y = (area.y + area.height as i32 - h).max(area.y);
    (x.clamp(area.x, max_x), y.clamp(area.y, max_y))
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

fn resize(win: &WebviewWindow, (w, h): (u32, u32)) -> Result<bool, String> {
    let scale = win.scale_factor().map_err(|e| e.to_string())?;
    let old_outer = win.outer_position().map_err(|e| e.to_string())?;
    let old_inner = win.inner_position().map_err(|e| e.to_string())?;
    let old_size = win.inner_size().map_err(|e| e.to_string())?;
    let next = PhysicalSize::new(
        (w as f64 * scale).round() as u32,
        (h as f64 * scale).round() as u32,
    );
    if next == old_size {
        return Ok(false);
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
        .map_err(|e| e.to_string())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{clamp_into, size_for};
    use crate::place::Rect;

    /// 尺寸表是 §2.1 那张表的抄本，钉住免得改一处就和文档对不上。
    #[test]
    fn the_four_size_slots_match_the_design_table() {
        assert_eq!(size_for("today"), (360, 480));
        assert_eq!(size_for("records"), (520, 680));
        assert_eq!(size_for("stats"), (520, 680));
        assert_eq!(size_for("collapsed"), (360, 169));
        assert_eq!(size_for("bar"), (360, 46));
    }

    /// 锚点补偿只在屏幕中间随便挪；挨着边时宁可让鼠标偏一点，也不能把卡片推到屏外。
    #[test]
    fn the_anchor_shift_stays_inside_the_work_area() {
        let area = Rect {
            x: 100,
            y: 50,
            width: 1000,
            height: 800,
        };
        // 工作区正中间：原样照挪。
        assert_eq!(clamp_into(area, (400, 600), (500, 200)), (500, 200));
        // 往右下挪过头：贴住工作区右下角。
        assert_eq!(clamp_into(area, (400, 600), (1200, 900)), (700, 250));
        // 往左上挪过头：贴住工作区左上角。
        assert_eq!(clamp_into(area, (400, 600), (-50, -80)), (100, 50));
        // 窗口比工作区还大：左上角对齐，不出现负的可用范围。
        assert_eq!(clamp_into(area, (1200, 900), (400, 400)), (100, 50));
    }
}
