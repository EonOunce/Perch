//! 窗口位置的两套坐标（DESIGN §3.4）。
//!
//! `settings.json` 里存的是「显示器名 + 相对该显示器工作区左上角的逻辑坐标」：
//! 换分辨率、改缩放，或者把卡片从小屏拖到大屏，都要能落回看得见的地方。
//! 物理像素与逻辑坐标的换算只在这里做——125% 缩放下混用一次，卡片就偏掉五分之一屏。

/// 物理像素矩形：工作区与窗口外框都用它描述。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// 物理像素 → 相对工作区左上角的逻辑坐标。
pub fn to_relative(area: Rect, scale: f64, pos: (i32, i32)) -> (f64, f64) {
    (
        (pos.0 - area.x) as f64 / scale,
        (pos.1 - area.y) as f64 / scale,
    )
}

/// 逻辑坐标 → 物理像素，并把整个窗口夹回工作区：
/// 显示器变窄、任务栏变高之后，记忆下来的位置也不能把卡片送到屏幕外。
pub fn from_relative(area: Rect, scale: f64, size: (u32, u32), rel: (f64, f64)) -> (i32, i32) {
    let x = area.x + (rel.0 * scale).round() as i32;
    let y = area.y + (rel.1 * scale).round() as i32;
    (
        x.clamp(
            area.x,
            (area.x + area.width as i32 - size.0 as i32).max(area.x),
        ),
        y.clamp(
            area.y,
            (area.y + area.height as i32 - size.1 as i32).max(area.y),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1040,
    };

    #[test]
    fn a_125_percent_screen_stores_logical_not_physical_coords() {
        let rel = to_relative(SCREEN, 1.25, (1250, 400));
        assert_eq!(rel, (1000.0, 320.0));
        assert_eq!(from_relative(SCREEN, 1.25, (450, 600), rel), (1250, 400));
    }

    #[test]
    fn a_monitor_left_of_the_primary_keeps_negative_origins() {
        // 副屏在主屏左边时工作区原点是负的；相对坐标仍从它自己的左上角起算。
        let left = Rect {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1040,
        };
        let rel = to_relative(left, 1.0, (-1000, 200));
        assert_eq!(rel, (920.0, 200.0));
        assert_eq!(from_relative(left, 1.0, (360, 480), rel), (-1000, 200));
    }

    #[test]
    fn a_position_beyond_the_new_work_area_is_pulled_back() {
        // 换过显示器或任务栏变高：记忆的位置比现在的工作区更靠右下。
        assert_eq!(
            from_relative(SCREEN, 1.0, (360, 480), (1400.0, 900.0)),
            (1400, 560)
        );
    }

    #[test]
    fn a_window_bigger_than_the_work_area_sticks_to_the_origin() {
        let tiny = Rect {
            x: 0,
            y: 0,
            width: 320,
            height: 200,
        };
        assert_eq!(from_relative(tiny, 1.0, (520, 680), (100.0, 100.0)), (0, 0));
    }
}
