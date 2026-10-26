//! 桌面层嵌入探针：把卡片窗口挂到 Explorer 的桌面层（WorkerW / Progman）下面。
//! 对应 DESIGN §5.2 方案 A；实测结论写进 `docs/desktop-layer-spike.md`。
//! 拿不到桌面窗口时保持普通窗口（方案 B），产品功能不受影响。

use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITOR_DEFAULTTONEAREST, MONITORINFO,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ChildWindowFromPoint, CreateWindowExW, DefWindowProcW, DispatchMessageW, EnumWindows,
    FindWindowExW, FindWindowW, GA_PARENT, GetAncestor, GetClassNameW, GetWindowLongPtrW,
    GetMessageW, GetWindowRect, GetClientRect, HWND_TOP, IsWindow, IsWindowVisible, MSG, PostQuitMessage,
    RegisterClassExW, RegisterWindowMessageW, PostMessageW, SendMessageTimeoutW, SetParent, SetWindowLongPtrW,
    SetWindowPos, TranslateMessage, WindowFromPoint, CS_HREDRAW, GWL_EXSTYLE, GWL_STYLE,
    SMTO_ABORTIFHUNG, SWP_NOACTIVATE, SWP_SHOWWINDOW, WNDCLASSEXW, WS_CHILD, WS_CLIPSIBLINGS,
    WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE, WM_DESTROY, WM_POWERBROADCAST,
};

use crate::probe_log;

/// Explorer 收到这条消息后会创建承载图标的 WorkerW 层。非公开协议，不保证各版本一致。
const WM_SHELLDLL_DESKTOP: u32 = 0x052C;
const PROGMAN: &str = "Progman";
const WORKERW: &str = "WorkerW";
const DEF_VIEW: &str = "SHELLDLL_DefView";
const WATCH_CLASS: &str = "PerchLayerWatch";
const PBT_APMSUSPEND: WPARAM = 4;
const PBT_APMRESUMESUSPEND: WPARAM = 7;
const PBT_APMRESUMEAUTOMATIC: WPARAM = 18;

pub const DEFAULT_MARGIN: i32 = 24;

/// 卡片 HWND 与当前父窗口，交给看门狗线程与心跳线程读取。
static CARD: AtomicUsize = AtomicUsize::new(0);
static PARENT: AtomicUsize = AtomicUsize::new(0);
/// 嵌入前的屏幕矩形，用于还原。
static SAVED: Mutex<Option<Rect>> = Mutex::new(None);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// 挂在 WorkerW 上（桌面图标层之后的那一层）。
    WorkerW,
    /// 没找到 WorkerW，直接挂 Progman。仍在桌面层，但可能与桌面图标重叠。
    Progman,
    /// 拿不到桌面窗口，保持普通窗口。
    Plain,
}

impl Layer {
    pub fn label(self) -> &'static str {
        match self {
            Layer::WorkerW => "WorkerW",
            Layer::Progman => "Progman",
            Layer::Plain => "普通窗口",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    fn from_windows(r: &RECT) -> Rect {
        Rect {
            x: r.left,
            y: r.top,
            w: r.right - r.left,
            h: r.bottom - r.top,
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn class_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    if n <= 0 {
        return "?".to_string();
    }
    String::from_utf16_lossy(&buf[..n as usize])
}

unsafe fn rect_of(hwnd: HWND) -> Option<Rect> {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if GetWindowRect(hwnd, &mut r) == 0 {
        None
    } else {
        Some(Rect::from_windows(&r))
    }
}

fn card() -> HWND {
    CARD.load(Ordering::SeqCst) as HWND
}

/// 缓存的卡片句柄。嵌入之后 `FindWindowW` 按标题就找不到它了（只枚举顶层窗口），
/// 所以后续一切操作都必须用这里绑定的句柄。
pub fn card_hwnd() -> Option<HWND> {
    let h = card();
    if h.is_null() {
        None
    } else {
        Some(h)
    }
}

/// 卡片当前的屏幕矩形（嵌入后也是屏幕坐标，GetWindowRect 一贯如此）。
pub unsafe fn card_rect() -> Option<Rect> {
    let h = card();
    if h.is_null() {
        None
    } else {
        rect_of(h)
    }
}

/// 当前的桌面层父窗口；未嵌入时 None。抓它可以看到自己这一层被合成成什么样。
pub fn parent_hwnd() -> Option<HWND> {
    let h = parent();
    if h.is_null() {
        None
    } else {
        Some(h)
    }
}

fn parent() -> HWND {
    PARENT.load(Ordering::SeqCst) as HWND
}

/// 找到应当承载卡片的桌面窗口，返回 (父窗口, 走的是哪条路径)。
unsafe fn find_desktop_parent() -> (Option<HWND>, String) {
    let progman = FindWindowW(wide(PROGMAN).as_ptr(), std::ptr::null());
    if progman.is_null() {
        return (None, "找不到 Progman".to_string());
    }
    let mut result: usize = 0;
    let sent = SendMessageTimeoutW(
        progman,
        WM_SHELLDLL_DESKTOP,
        0,
        0,
        SMTO_ABORTIFHUNG,
        1_000,
        &mut result,
    );
    if sent == 0 {
        probe_log::note("[layer] 0x052C 未被处理（Explorer 忙或不支持），继续找现有窗口");
    }

    // 路径 1：DefView 还在 Progman 里 → 用 Progman 之后的那个同级 WorkerW。
    if !FindWindowExW(progman, null_mut(), wide(DEF_VIEW).as_ptr(), std::ptr::null()).is_null() {
        let sibling = FindWindowExW(null_mut(), progman, wide(WORKERW).as_ptr(), std::ptr::null());
        if !sibling.is_null() {
            return (
                Some(sibling),
                "DefView∈Progman，取 Progman 之后的同级 WorkerW".to_string(),
            );
        }
        return (
            Some(progman),
            "DefView∈Progman，没有同级 WorkerW，用 Progman".to_string(),
        );
    }

    // 路径 2：DefView 已搬进某个 WorkerW → 取那个 WorkerW 之后的同级 WorkerW。
    struct Scan {
        host: HWND,
    }
    unsafe extern "system" fn probe(hwnd: HWND, lparam: LPARAM) -> BOOL {
        if !FindWindowExW(hwnd, null_mut(), wide(DEF_VIEW).as_ptr(), std::ptr::null()).is_null() {
            (*(lparam as *mut Scan)).host = hwnd;
            return 0;
        }
        1
    }
    let mut scan = Scan { host: null_mut() };
    EnumWindows(
        Some(probe as unsafe extern "system" fn(HWND, LPARAM) -> BOOL),
        &mut scan as *mut Scan as LPARAM,
    );
    if scan.host.is_null() {
        return (None, "两条路径都没找到 SHELLDLL_DefView".to_string());
    }
    let host_class = class_of(scan.host);
    let sibling = FindWindowExW(null_mut(), scan.host, wide(WORKERW).as_ptr(), std::ptr::null());
    if !sibling.is_null() {
        return (
            Some(sibling),
            format!("DefView∈{host_class}，取其后继 WorkerW"),
        );
    }
    let layer = if host_class == WORKERW {
        Layer::WorkerW
    } else {
        Layer::Progman
    };
    (
        Some(scan.host),
        format!(
            "DefView∈{host_class}，没有后继窗口，直接用它（判为 {}）",
            layer.label()
        ),
    )
}

/// 挂到桌面层。已经挂过则先还原再重挂（Explorer 重启、唤醒后走这条路）。
pub unsafe fn embed(margin: i32) -> Layer {
    let card_hwnd = card();
    if card_hwnd.is_null() || IsWindow(card_hwnd) == 0 {
        probe_log::note("[layer] 卡片窗口无效，跳过嵌入");
        return Layer::Plain;
    }
    if parent() != 0 as *mut c_void {
        unembed();
    }
    let dpi_before = GetDpiForWindow(card_hwnd);
    let Some(before) = rect_of(card_hwnd) else {
        probe_log::note("[layer] 读不到卡片矩形，跳过嵌入");
        return Layer::Plain;
    };
    let (found, how) = find_desktop_parent();
    let Some(new_parent) = found else {
        probe_log::note(format!("[layer] 回退普通窗口：{how}"));
        return Layer::Plain;
    };
    let parent_class = class_of(new_parent);
    let layer = if parent_class == WORKERW {
        Layer::WorkerW
    } else {
        Layer::Progman
    };
    probe_log::note(format!(
        "[layer] {how} → 父窗口 {new_parent:?} 类名={parent_class}；卡片嵌入前 {before:?} dpi={dpi_before}"
    ));

    *SAVED.lock().unwrap() = Some(before);
    PARENT.store(new_parent as usize, Ordering::SeqCst);

    let style = GetWindowLongPtrW(card_hwnd, GWL_STYLE) as u32;
    let child_style = (style & !(WS_POPUP | WS_CHILD)) | WS_CHILD | WS_CLIPSIBLINGS;
    SetWindowLongPtrW(card_hwnd, GWL_STYLE, child_style as isize);
    if SetParent(card_hwnd, new_parent).is_null() {
        probe_log::note("[layer] SetParent 返回空：还原样式，按普通窗口继续");
        SetWindowLongPtrW(card_hwnd, GWL_STYLE, style as isize);
        PARENT.store(0, Ordering::SeqCst);
        *SAVED.lock().unwrap() = None;
        return Layer::Plain;
    }

    let mut target = desktop_position(new_parent, card_hwnd, before, margin);
    if let Some(force) = take_forced_rect() {
        let p = rect_of(new_parent).unwrap_or(Rect { x: 0, y: 0, w: 0, h: 0 });
        target = Rect { x: force.x - p.x, y: force.y - p.y, w: force.w, h: force.h };
        probe_log::note(format!("[layer] 按实测要求强落屏幕矩形 {force:?} → 父内 {target:?}"));
    }
    let moved = SetWindowPos(
        card_hwnd,
        HWND_TOP,
        target.x,
        target.y,
        target.w,
        target.h,
        SWP_NOACTIVATE | SWP_SHOWWINDOW,
    );
    probe_log::note(format!(
        "[layer] SetWindowPos={} 父内坐标 {target:?} 屏幕矩形 {:?} dpi={}",
        moved != 0,
        rect_of(card_hwnd),
        GetDpiForWindow(card_hwnd)
    ));
    log_style_snapshot(card_hwnd);
    layer
}

/// 还原成普通窗口。退出前必须调用，否则会留下一个看不见也抓不到的子窗口。
pub unsafe fn unembed() {
    let card_hwnd = card();
    let old_parent = PARENT.swap(0, Ordering::SeqCst);
    if old_parent == 0 || card_hwnd.is_null() || IsWindow(card_hwnd) == 0 {
        return;
    }
    let saved = SAVED.lock().unwrap().take();
    SetParent(card_hwnd, null_mut());
    let style = GetWindowLongPtrW(card_hwnd, GWL_STYLE) as u32;
    let popup_style = (style & !(WS_CHILD | WS_CLIPSIBLINGS)) | WS_POPUP;
    SetWindowLongPtrW(card_hwnd, GWL_STYLE, popup_style as isize);
    if let Some(r) = saved {
        SetWindowPos(
            card_hwnd,
            HWND_TOP,
            r.x,
            r.y,
            r.w,
            r.h,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        probe_log::note(format!("[layer] 已还原为普通窗口，回到 {r:?}"));
    } else {
        probe_log::note("[layer] 已还原为普通窗口（没有记下原位置，交给窗口管理）");
    }
}

/// 卡片在父窗口内的坐标：贴该显示器工作区右下角。子窗口坐标相对父窗口客户区原点。
unsafe fn desktop_position(parent_hwnd: HWND, card_hwnd: HWND, saved: Rect, margin: i32) -> Rect {
    let Some(parent_rect) = rect_of(parent_hwnd) else {
        return Rect {
            x: margin,
            y: margin,
            w: saved.w,
            h: saved.h,
        };
    };
    let monitor = MonitorFromWindow(card_hwnd, MONITOR_DEFAULTTONEAREST);
    let mut info: MONITORINFO = std::mem::zeroed();
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(monitor, &mut info) == 0 {
        return Rect {
            x: parent_rect.w - saved.w - margin,
            y: parent_rect.h - saved.h - margin,
            w: saved.w,
            h: saved.h,
        };
    }
    Rect {
        x: info.rcWork.right - parent_rect.x - saved.w - margin,
        y: info.rcWork.bottom - parent_rect.y - saved.h - margin,
        w: saved.w,
        h: saved.h,
    }
}

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

/// 测试用：强制下一次嵌入落到这个屏幕矩形（生产里桌面层就贴右下角，没有这个需求）。
static FORCE_RECT: Mutex<Option<Rect>> = Mutex::new(None);

pub fn set_forced_rect(r: Rect) {
    *FORCE_RECT.lock().unwrap() = Some(r);
}

fn take_forced_rect() -> Option<Rect> {
    FORCE_RECT.lock().unwrap().take()
}

/// 遮挡统计的中转：`rects` 是真会压住桌面的顶层窗口，`skipped` 用来核对为什么某个窗口没算进来。
struct Occlusion {
    rects: Vec<(String, Rect)>,
    skipped: Vec<String>,
}

unsafe extern "system" fn collect_blockers(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let out = &mut *(lparam as *mut Occlusion);
    if hwnd == card() {
        return 1;
    }
    let cls = class_of(hwnd);
    if cls == PROGMAN || cls == WORKERW || cls == "Shell_TrayWnd" {
        return 1;
    }
    if IsWindowVisible(hwnd) == 0 {
        out.skipped.push(format!("{cls}:不可见"));
        return 1;
    }
    if (GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32) & WS_EX_TOOLWINDOW != 0 {
        out.skipped.push(format!("{cls}:工具窗口"));
        return 1;
    }
    match rect_of(hwnd) {
        // 最小化的窗口落在 -32000，不在工作区里，不算遮挡。
        Some(r) if r.w > 0 && r.h > 0 && r.y > -1000 => out.rects.push((cls, r)),
        Some(r) => out.skipped.push(format!("{cls}:矩形{r:?}")),
        None => out.skipped.push(format!("{cls}:读不到矩形")),
    }
    1
}

/// 在工作区里找一个能放下 w×h、且不被任何可见顶层窗口压住的位置。
/// 只为「屏幕合成实测」服务：桌面上有别的窗口时，右下角根本看不见。
pub unsafe fn find_clear_rect(w: i32, h: i32) -> Option<Rect> {
    let mut occ = Occlusion {
        rects: Vec::new(),
        skipped: Vec::new(),
    };
    EnumWindows(
        Some(collect_blockers as unsafe extern "system" fn(HWND, LPARAM) -> BOOL),
        &mut occ as *mut Occlusion as LPARAM,
    );
    let mut seen: Vec<String> = Vec::new();
    for s in occ.skipped {
        if !seen.iter().any(|p| p == &s) {
            seen.push(s);
        }
    }
    probe_log::note(format!(
        "[clear] 算作遮挡 {} 个：{:?}；跳过 {} 种（最多列 20）：{:?}",
        occ.rects.len(),
        occ.rects,
        seen.len(),
        &seen[..seen.len().min(20)]
    ));
    let blockers = occ.rects;
    let monitor = MonitorFromWindow(card(), MONITOR_DEFAULTTONEAREST);
    let mut info: MONITORINFO = std::mem::zeroed();
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(monitor, &mut info) == 0 {
        return None;
    }
    let work = Rect::from_windows(&info.rcWork);
    let step = 40;
    let mut y = work.y;
    while y + h <= work.y + work.h {
        let mut x = work.x;
        while x + w <= work.x + work.w {
            let cand = Rect { x, y, w, h };
            if !blockers.iter().any(|(_, b)| overlaps(&cand, b)) {
                probe_log::note(format!("[clear] 找到空位 {cand:?}"));
                return Some(cand);
            }
            x += step;
        }
        y += step;
    }
    probe_log::note(format!("[clear] 装不下 {w}x{h}：屏幕合成实测只能对着被挡住的桌面做"));
    None
}

unsafe fn log_style_snapshot(hwnd: HWND) {
    let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
    probe_log::note(format!(
        "[style] 类名={} WS_CHILD={} WS_POPUP={} WS_VISIBLE={} IsWindowVisible={} GetAncestor(PARENT)={:?} GetAncestor(ROOT)={:?}",
        class_of(hwnd),
        style & WS_CHILD != 0,
        style & WS_POPUP != 0,
        style & WS_VISIBLE != 0,
        IsWindowVisible(hwnd) != 0,
        GetAncestor(hwnd, GA_PARENT),
        windows_sys::Win32::UI::WindowsAndMessaging::GetAncestor(
            hwnd,
            windows_sys::Win32::UI::WindowsAndMessaging::GA_ROOT
        )
    ));
    let mut c = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    GetClientRect(hwnd, &mut c);
    probe_log::note(format!(
        "[style] 窗口矩形={:?} 客户区={}x{}（两者之差是窗口边框，那点像素上的点击会落在窗口身上而不是桌面）",
        rect_of(hwnd),
        c.right,
        c.bottom
    ));
}

/// 一行汇总，给 JS 面板和心跳日志用。
pub unsafe fn status_line() -> String {
    let hwnd = card();
    if hwnd.is_null() {
        return "卡片窗口还没建好".to_string();
    }
    let p = parent();
    let layer = if p.is_null() {
        Layer::Plain
    } else if class_of(p) == WORKERW {
        Layer::WorkerW
    } else {
        Layer::Progman
    };
    format!(
        "层={} 父={p:?} 矩形={:?} dpi={} 可见={}",
        layer.label(),
        rect_of(hwnd),
        GetDpiForWindow(hwnd),
        IsWindowVisible(hwnd) != 0
    )
}

/// 命中测试旁证：卡片中心那一点系统会把鼠标交给谁、卡片自己的子窗口（WebView2）在不在命中链上。
/// 真正的手感还是要人眼看着点，但这条能排除「子窗口不参与 hit-test」这种硬故障。
pub unsafe fn hit_test_line() -> String {
    let hwnd = card();
    if hwnd.is_null() {
        return "卡片窗口还没建好".to_string();
    }
    let Some(r) = rect_of(hwnd) else {
        return "拿不到卡片矩形".to_string();
    };
    let describe = |h: HWND| -> String {
        if h.is_null() {
            "无".to_string()
        } else {
            format!("{h:?}/{}", class_of(h))
        }
    };
    let center = POINT {
        x: r.x + r.w / 2,
        y: r.y + r.h / 2,
    };
    let client = POINT {
        x: r.w / 2,
        y: r.h / 2,
    };
    let top = WindowFromPoint(center);
    let root_of_top = if top.is_null() {
        null_mut()
    } else {
        GetAncestor(top, windows_sys::Win32::UI::WindowsAndMessaging::GA_ROOT)
    };
    let child = ChildWindowFromPoint(hwnd, client);
    format!(
        "中心({},{}) WindowFromPoint={} 根={} 是卡片本身={}",
        center.x,
        center.y,
        describe(top),
        describe(root_of_top),
        root_of_top == hwnd
    ) + &format!("｜卡片内 ChildWindowFromPoint={}", describe(child))
}

fn taskbar_created_id() -> u32 {
    static ID: AtomicUsize = AtomicUsize::new(0);
    let cached = ID.load(Ordering::SeqCst);
    if cached != 0 {
        return cached as u32;
    }
    let id = unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) };
    ID.store(id as usize, Ordering::SeqCst);
    id
}

static WATCH: AtomicUsize = AtomicUsize::new(0);

/// 自己发一条 `TaskbarCreated` 给监听窗口，模拟 Explorer 重启后的广播。
/// 真去重启 explorer.exe 会关掉用户开着的资源管理器窗口，spike 不做这种事。
pub fn poke_shell_event() -> &'static str {
    let h = WATCH.load(Ordering::SeqCst) as HWND;
    if h.is_null() {
        return "监听窗口还没建好";
    }
    if unsafe { PostMessageW(h, taskbar_created_id(), 0, 0) } == 0 {
        return "投递失败";
    }
    "已投递 TaskbarCreated，看日志里有没有重挂"
}

unsafe extern "system" fn watch_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == taskbar_created_id() {
        probe_log::note("[watch] 收到 TaskbarCreated → 重新嵌入");
        embed(DEFAULT_MARGIN);
        return 0;
    }
    if msg == WM_POWERBROADCAST {
        let kind = if wparam == PBT_APMSUSPEND {
            "即将睡眠"
        } else if wparam == PBT_APMRESUMESUSPEND {
            "从睡眠恢复（用户唤醒）"
        } else if wparam == PBT_APMRESUMEAUTOMATIC {
            "自动恢复唤醒"
        } else {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        };
        probe_log::note(format!("[watch] 电源事件：{kind} → 重挂检查"));
        embed(DEFAULT_MARGIN);
        return 1;
    }
    if msg == WM_DESTROY {
        PostQuitMessage(0);
        return 0;
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// 子窗口收不到广播消息，所以要有一个自己的隐形顶层窗口来监听 Explorer 重启与电源事件。
pub fn start_watchdog() {
    std::thread::spawn(|| unsafe {
        let name = wide(WATCH_CLASS);
        let hinstance = GetModuleHandleW(std::ptr::null());
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW,
            lpfnWndProc: Some(watch_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: null_mut(),
            hCursor: null_mut(),
            hbrBackground: null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: name.as_ptr(),
            hIconSm: null_mut(),
        };
        if RegisterClassExW(&class) == 0 {
            probe_log::note("[watch] 注册监听窗口类失败：Explorer 重启后不会自动重挂");
            return;
        }
        // 不挂 WS_VISIBLE：需要真实顶层窗口才收得到广播，但不显示任何东西。
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            name.as_ptr(),
            name.as_ptr(),
            WS_POPUP,
            -4000,
            -4000,
            4,
            4,
            null_mut(),
            null_mut(),
            hinstance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            probe_log::note("[watch] 创建监听窗口失败");
            return;
        }
        WATCH.store(hwnd as usize, Ordering::SeqCst);
        probe_log::note(format!(
            "[watch] 监听线程就绪 hwnd={hwnd:?} TaskbarCreated={}",
            taskbar_created_id()
        ));
        let mut msg: MSG = std::mem::zeroed();
        loop {
            let got = GetMessageW(&mut msg, null_mut(), 0, 0);
            if got <= 0 {
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}

/// 每秒比对状态指纹，变了立刻记；另加 10 秒心跳，事后能还原时间线。
pub fn start_heartbeat() {
    std::thread::spawn(|| unsafe {
        let mut last = String::new();
        let mut beat = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(1000));
            let line = status_line();
            if line != last {
                probe_log::note(format!("[state] 变化 → {line}"));
                last = line.clone();
                beat = Instant::now();
            }
            if beat.elapsed() >= Duration::from_secs(10) {
                probe_log::note(format!("[beat] {line}"));
                beat = Instant::now();
            }
        }
    });
}

/// 记录卡片 HWND，供看门狗与心跳使用。
pub fn bind_card(hwnd: HWND) {
    CARD.store(hwnd as usize, Ordering::SeqCst);
}

/// 拆掉再重挂，用于在没有真的重启 Explorer 的情况下走一遍重挂路径。
pub unsafe fn cycle() -> &'static str {
    if card().is_null() {
        return "无卡片";
    }
    unembed();
    embed(DEFAULT_MARGIN).label()
}
