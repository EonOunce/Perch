# 桌面层 Spike 结论（DESIGN §5.2 方案 A）

日期：2026-10-27。代码在 `src-tauri/src/desktop_layer.rs` + `spike/ui/index.html`，是一次性探针，不进正式实现。
日志：`%LOCALAPPDATA%\Perch\spike.log`；抓图：`%LOCALAPPDATA%\Perch\shots\`。

## 1. 测试环境

| 项 | 值 |
| --- | --- |
| 系统 | Windows 11 23H2（10.0.22631） |
| 显示器 | 1920×1080，缩放 125%（`GetDpiForWindow` = 120），单屏 |
| 运行时 | Tauri 2.12.5 / wry / WebView2 155.x / windows-sys 0.59 |
| 窗口配置 | `decorations:false`、`transparent:true`、`alwaysOnTop:false`、`skipTaskbar:true`、360×480 logical |

## 2. 结论一句话

**方案 A 在本机（装了壁纸引擎 + 腾讯桌面整理）不可行，正式实现走方案 B**（2026-10-27 复测，见 §7）。
原因不是"挂不上去"，而是挂上去之后**看得见与点得着不能同时得到**：桌面层最顶的位置被第三方软件占着。
§3 的原始记录保留在下面——那批前提（句柄只认一次、父窗口销毁要重建、坐标按父窗口相对、热区按客户区算）
在方案 A 重新启用时仍然成立，只是方案 B 用不上。

## 3. 逐项实测证据

### 3.1 找父窗口
这台机器走的是路径 2：`SHELLDLL_DefView` 已经在某个 WorkerW 里，取它**之后的同级 WorkerW**（`0x301d6`）。
`SendMessageTimeoutW(Progman, 0x052C, …)` 有响应。两条路径都实现了，拿不到时回退普通窗口。

### 3.2 挂上去之后的窗口状态
```
[layer] SetWindowPos=true 父内坐标 Rect { x: 1428, y: 386, w: 468, h: 610 } 屏幕矩形 Some(同上) dpi=120
[style] 类名=Tauri Window WS_CHILD=true WS_POPUP=false WS_VISIBLE=true IsWindowVisible=true
        GetAncestor(PARENT)=0x301d6 GetAncestor(ROOT)=0x301d6
```
- 样式换成功（`WS_POPUP` → `WS_CHILD | WS_CLIPSIBLINGS`），`SetParent` 没返回空。
- 位置正确：工作区右下角留 24 物理像素边距。
- **DPI 没被改变**（嵌入前后都是 120），这条是 §5.2 里担心的"子窗口丢 DPI"风险，实测没发生。
- **窗口比客户区大**：`窗口矩形 468×610`、`客户区 450×600`。360×480 logical × 1.25 = 450×600 才是网页可视区，
  四周多出来的约 9px 是非客户区边框。含义：鼠标落在那一圈上走的是系统边框行为，不是网页点击；
  正式实现算位置、算点击热区要用客户区，别拿 `GetWindowRect` 当卡片尺寸。

### 3.3 WebView2 到底有没有画出来（核心风险）
屏幕上别的窗口把桌面挡满了，所以用两种绕开遮挡的办法：

1. **抓卡片自己**：`PrintWindow(card, …, PW_RENDERFULLCONTENT)` → 99.7% 像素有内容，平均色 `#e6e1d8`（纸色混着透明角）。
2. **抓父窗口**（决定性）：`PrintWindow(WorkerW, …)` 得到整块桌面层 1920×1080，卡片落在 `(1428,386)` 的位置，
   中心 24×24 平均色 **`#f6f1e7`** —— 正是 CSS 里写死的纸色。图里能读出头「栖迟 · 桌面层探针」、六个按钮、
   以及日志行 `probe_status → 层=WorkerW 父=0x301d6 矩形=Some(Rect { x: 1428, y: 386, w: 468, h: 610 }) dpi=120 可见=true`。
   文件：`shots/perch_07_parent_layer.bmp`。

第 2 条同时证明了三件事：**桌面层里合成正常**（透明角透出壁纸，没有黑块）、**WebView2 在子窗口里持续渲染**、
**JS 与 Rust 的 invoke 往返在嵌入状态下仍然工作**（那行日志是页面里 3 秒一次的定时器在嵌入之后打出来的）。

### 3.4 鼠标命中
- 卡片内 `ChildWindowFromPoint(中心)` → `WRY_WEBVIEW`，WebView2 的子窗口在命中链上。
- 嵌入后 `WindowFromPoint(卡片中心)` → 另一个应用的 `Chrome_WidgetWin_1`（Edge），**不是**卡片。
  同一位置还原成普通窗口后再抓屏，中心就是 `#efeae0`（卡片本体）。
  这正是"桌面层不是置顶"的预期行为：有窗口压着时它就在下面。
- 真正的点击/打字手感没测（见 §5）。

### 3.5 Explorer 广播后的重挂
自己 `PostMessageW(watch, TaskbarCreated)` 模拟了一次广播：
```
[watch] 收到 TaskbarCreated → 重新嵌入
[layer] 已还原为普通窗口，回到 Rect { x: 1446, y: 396, … }
[layer] DefView∈WorkerW，取其后继 WorkerW → 父窗口 0x301d6 …
[style] … WS_CHILD=true …
```
还原 → 重挂 → 落回同一个位置，全程在监听线程（非窗口所属线程）上完成，没崩、没卡。
电源事件 `WM_POWERBROADCAST` 走的是同一个 `embed()` 入口，逻辑同样，但真睡眠没测。

### 3.6 挂着不动会不会自己掉
嵌入后让它自己跑了约 4 分钟：心跳每 10 秒一行，全程 `层=WorkerW 父=0x301d6 矩形=(1428,386,468,610) dpi=120 可见=true`
没有变化（状态指纹只在嵌入时变过一次），进程内存约 72 MB，没有白屏、没有掉回普通窗口。
一整天级别的观察还在她的待测清单里。

## 4. 正式实现必须吸收的五条
1. **句柄只认一次**：挂进 WorkerW 之后，`FindWindowW(NULL, title)` **找不到**这张卡片了（它只枚举顶层窗口）。
   探针里第一次跑就踩了这个坑（日志 `[shot:02_embedded] 找不到卡片窗口`）。
   正式实现要在建窗时拿一次 `window.hwnd()` 存下来，之后所有操作都用缓存句柄，并且用 `IsWindow` 判活。
2. **父窗口没了 = 卡片窗口也没了**：Win32 里子窗口随父窗口销毁。真的重启 Explorer 时那个 WorkerW 会被销毁，
   卡片大概跟着一起没了，"收到广播后重挂"救不回来 —— 得**重建窗口再挂**。这条本机没实测（不打算为了测试去重启用户的 Explorer）。
   Phase 1 的做法：监听线程发现 `IsWindow(card)==false` 时通知主线程用 `WebviewWindowBuilder` 重建一张，然后重挂。
3. **坐标是父窗口相对**：子窗口坐标原点 = 父窗口原点。这台机器 WorkerW 起点是 (0,0) 所以看着和屏幕坐标一样，
   不能当成普遍规律；正式实现要像探针一样先取父窗口矩形再减。
4. **拿不到桌面层就普通窗口**：`embed()` 返回 `Layer::Plain` 时产品功能完全不受影响，只是不在桌面上。
   这条回退要保留，别做成"挂不上就报错退出"。
5. **卡片尺寸按客户区算**：`GetWindowRect` 比网页可视区四周各大几个像素（本机 468×610 vs 客户区 450×600），
   那一圈是非客户区。摆位置、判热区、跟 §2.1 的 360×480 对齐时都用客户区。

## 5. 还需要她本人看的（我做不了）

| 待测 | 怎么测 | 判据 |
| --- | --- | --- |
| 桌面层里的点击与打字 | 空桌面（Win+D）后点卡片上的按钮、在输入框打字 | 有反应、光标进输入框 |
| 拖动 | 按住卡片顶部标题区拖动 | 跟手、不卡、不越界、松手停住 |
| 真·Explorer 重启 | 任务管理器结束 `explorer.exe` 再文件→运行新任务 | 卡片是否还在；不在的话多久回来 |
| 睡眠唤醒 | 合盖/睡眠后再唤醒 | 卡片是否还在原位、是否还在桌面上 |
| 150% 缩放 / 双屏 | 换到那台机器或改缩放 | 尺寸与位置是否按 DPI 走、跨屏是否跟对显示器 |
| 长跑 | 挂着用一整天 | 内存不涨、卡片不白屏 |

## 7. 复测：正式前端接上之后，方案 A 被判不可行（同日）

前端（`src/`）接上、Rust 写通道打通之后，第一次在真机上让她操作，反馈是「卡片看得见但点不动」。
用 `WindowFromPoint` + `EnumChildWindows` 直接量桌面层的 z 序，本机 672 个顶层窗口里有 **19 个 `Progman`/`WorkerW` 候选**，
其中真正有用的三个：

| 层 | 扩展样式 | 直接子窗口（从前到后） | 卡片挂进去的结果 |
| --- | --- | --- | --- |
| `WorkerW/0x501ba` | `WS_EX_TOOLWINDOW` | `TXMiniSkin`(1920×1079) → `SHELLDLL_DefView` → `SysListView32` | **点得着、看不见**：腾讯桌面整理的面板压在我们上面，且 1 秒内会抢回去 |
| `WorkerW/0x301d6` | **`WS_EX_TRANSPARENT`** + `WS_EX_NOACTIVATE` | `WPEDesktopDX11Window`（壁纸引擎） | **看得见、点不着**：穿透层在鼠标派发时整棵子树都不存在 |
| `Progman/0x1025a` | 无 | （空，DefView 已搬走） | 在所有桌面层之下，卡片会被壁纸盖住 |

旧代码按 z 序取「图标层宿主之后的同级 WorkerW」，在有实时壁纸软件的机器上取到的正是第二行那个穿透层。
改成「筛掉穿透层、挂进图标层宿主并置为最上子窗口」之后（`pick_host()` + 5 个单元测试），症状从「点不动」变成「看不见」——
`SetWindowPos(HWND_TOP)` 确实把我们抬到了 `SHELLDLL_DefView` 之上，但 `TXMiniSkin` 又压回我们上面。
**要赢只能去改别人家窗口的 z 序或样式，这个不能拿来做产品**，于是按 §5.2 的预授权落回方案 B。

方案 B 实测（普通窗口，启动时 `SetWindowPos(HWND_BOTTOM)`，`alwaysOnTop:false`）：

- 她本人实机确认：**最小化别的窗口、按 `Win+D` 之后卡片都还在**，并且**点得动**（打卡按钮有反应）。
- 同一张卡片被 WPS 的最大化窗口正常盖住：`WindowFromPoint` 四个点全部归 `OpusApp`(WPS)，符合「桌面层不是置顶」。
- 心跳日志：`层=普通窗口 父=0x0 根=…/Tauri Window 穿透=false`。
- **`skipTaskbar` 只摘任务栏按钮，不改变样式位**（2026-10-28 查明）：`GetWindowLong(GWL_EXSTYLE)` 读回来是
  `0x00040110` = `WS_EX_APPWINDOW|WS_EX_WINDOWEDGE|WS_EX_ACCEPTFILES`，既没有 `WS_EX_TOOLWINDOW(0x80)`
  也没有 `WS_EX_NOACTIVATE`。tao 0.37 的 `set_skip_taskbar` 只调 `ITaskbarList::DeleteTab`；无父窗口的窗口在 tao
  里固定带 `ON_TASKBAR`，每次重算样式都会把 `WS_EX_APPWINDOW` 写回来。UI 自动化枚举任务栏按钮确认**没有栖迟的按钮**，
  但 Alt+Tab 仍会把它当成可切换窗口列出来，窗口标题也还是探针期的 `PerchLayerProbe`。
  v0.7.3 起主卡片与快速记录窗都补上 `WS_EX_TOOLWINDOW`、去掉 `WS_EX_APPWINDOW`，并用子类过程在 `WM_STYLECHANGED`
  里守住（否则 `show()`/换置顶会把样式重算回去）；主窗口标题同时改成「栖迟」。改样式前后客户区尺寸、圆角与透明角均无变化。
- **还没测**：打字（需要焦点）、拖动、真·Explorer 重启、睡眠唤醒、150%/双屏、长跑。

代码入口：`desktop_layer::embed_requested()`（`PERCH_DESKTOP_EMBED=1` 才试方案 A，方便换机器复测）、
`desktop_layer::settle_bottom()`、`desktop_layer::reassert()`（看门狗与电源事件按当前模式落层，不会偷偷把卡片塞回桌面层）。

## 8. 探针自身的坑（留给下一个读代码的人）

- `PrintWindow` 必须带 `PW_RENDERFULLCONTENT`，否则 WebView2 内容区是空的；windows-sys 里这个常量在 `Win32_Storage_Xps` feature 下。
- 手写 BMP 时 `biHeight` 用负数（自上而下）会被一些看图软件解释反（Pillow 直接上下颠倒显示）。
  现在按标准自下而上写行序，看图一致。
- 抓屏（`BitBlt`）读的是最终合成结果，所以"卡片被别的窗口挡住"会照实拍进去，不能当成渲染失败的证据。
- `PrintWindow` 抓的是窗口重定向表面，**窗口在屏上没画出来也能抓到内容**——只用它验证「可见」会得到假阳性，
  必须配合真实屏幕抓取（`PIL.ImageGrab` + `SetProcessDpiAwareness(2)`）。
- 主窗标题已从 `PerchLayerProbe` 改成「栖迟」（v0.7.3）：按旧标题找窗口的探针脚本要改按类名或新标题找。
  控制台编码还会把中文标题显示成乱码，比较标题时用 `[Console]::OutputEncoding` 设 UTF8，或干脆按进程枚举。

## 9. 窗口外沿的 1px 直角线（2026-10-27）

用户反馈：卡片四周已经是 22px 圆角，最外沿却仍有一条线，在圆角外面露出直角。
量化：`PrintWindow` 第 0 行原先是 (243,243,243) 的一条浅灰线；真实屏幕抓取在客户区外沿看到一圈 1px 方框。
那是 DWM 沿窗口矩形画的边框线（`decorations:false` 只去掉标题栏绘制，边框线还在）。

**错误修法（已回退，留作教训）**：清掉样式位里的 `WS_CAPTION`。用户实机截图证明它更糟——
tao 的透明客户区扩展依赖原有样式位，清掉之后非客户区不再透明，被画成**一圈白框**，
`WS_SYSMENU` 残留还把系统菜单的**关闭按钮 X** 露在右上角；卡片内容倒是照常画。
结论：这个窗口的样式位一个都不要动。

**正确修法**：`desktop_layer::hide_dwm_border()` 只调
`DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE)`，让 DWM 不画那圈线，
在 `show()` 之后调用（`ui_ready` 与 3 秒看门狗两条路都走）。日志 `[frame] 去 DWM 边框 hr=0x00000000`。

修后验证（devdata 隔离实例 + 按进程号真实屏幕抓取）：圆角外干净、无白框、无 X、无 1px 线，
窗口几何不变（125% 下窗口 468×610、客户区 450×600）。

## 10. 顶部残留的 1px 标题栏线（2026-10-27）

用户截图：边框线没了，但卡片**最顶上**还有一条白线横着。
量化：真实屏幕抓取第 0 行是 (244,244,237)，桌面色在别处；那是 tao 的 `WS_CAPTION` 留下的
1px 非客户区残边，样式位不能动（§9 的教训），只能让 DWM 把客户区扩满整窗：
`DwmExtendFrameIntoClientArea(hwnd, MARGINS{-1,-1,-1,-1})`。

**时序陷阱**：开机 `show()` 前后立刻调扩展，窗口会整块不在屏上合成（日志 hr=0 但抓屏抓不到卡）。
改成 `show()` 之后延迟 1.5s 再调（`schedule_frame_extend`，`Once` 保证只一次）。
验证：devdata 实例 + 真实屏幕抓取 `target/topline-check.png`——顶部圆角直接接桌面色，白线消失，
卡片其余部分正常；`PrintWindow` 在这里会假阳性（它抓的是重定向表面，DWM 没合成也照样有内容），
结论一律以真实抓屏为准。

## 11. 托盘（2026-10-28，方案 B 下首次实测）

做法：不用 `probe_tray` 命令走捷径，而是**真实鼠标点击**托盘图标——
UI Automation 枚举 `Shell_TrayWnd` 与 `TopLevelWindowForOverflowXamlIsland` 拿到图标矩形，
`user32.mouse_event` 按中心点按下/抬起，再用 WebView2 CDP 读窗口状态。
脚本 `target/tray-flyout.ps1`（PS 5.1，只能 ASCII，中文用 `\u6816\u8FDF` 这类正则转义匹配）。

- **图标存在但默认不露出**：Win11 把第三方图标收进「显示隐藏的图标」浮窗里，
  常驻通知区只有 OneDrive 这类被系统提升过的项。枚举结果 `name=[栖迟] id=[NotifyItemIcon]`，
  位置在浮窗第一格（125% 缩放下物理 1428,798 50×50）。
  浮窗要先点 `显示隐藏的图标`（1533,1020）才存在，所以枚举脚本必须「点开→再枚举」，
  且第一次点可能把它关掉，需要重试一次。
  **结论：功能没问题，但新装用户可能找不到图标。** 后续要么在设置里提示「右键任务栏 → 任务栏设置 →
  其他系统托盘图标」放行，要么在首次启动引导里说一句；不改代码去强推（没有受支持的 API）。
- **左键短时提到前台**：真实左键点击后 `alwaysOnTop=true`，8 秒后自动回到 `false`，
  日志里心跳仍是 `层=普通窗口`。符合 §5.2「桌面层不是置顶」——托盘左键只许短时提一下。
  多次点击用 `Raises` 代际计数，被取代的旧计时器不会把新的一次提前的收回去（单测覆盖）。
- **右键菜单**：真实右键点击弹出原生菜单 `#32768`，条目为「快速记录」+「显示 / 隐藏卡片」+ 分隔符 +「退出」，
  浅色渲染正常（`target/tray-menu.png`）。
- **菜单项真实点击**：点「显示 / 隐藏卡片」→ `visible=false`；再点一次 → `visible=true`。
  「退出」没点（会关掉正在验证的实例），其处理分支与已验证的 toggle 走同一个 `on_menu_event` match。
- **图标外观**：用的是 `app.default_window_icon()`，在 16px 下是一枚深色圆角方块中间一个白点，
  认得出但不是为托盘画的小图。留作后续（单色小图在深浅任务栏上都更稳）。
- **构建前提**：`tauri` 需要 `tray-icon` feature，否则 `tauri::tray` 整个模块不存在。
  本机 tray-icon 0.25.1 + muda 0.20.0 已在 `Cargo.lock` 与本地 registry 缓存里，离线可编。
  托盘挂不上只记日志、不影响启动（`shell::build` 返回 `()`）。

## 12. 全局快捷键与快速记录小窗（2026-10-28）

做法：不装 `tauri-plugin-global-shortcut`（本地 registry 缓存里没有它，离线构建装不上），
用 `windows-sys` 已有的 `RegisterHotKey` + 一个消息专用窗口（`HWND_MESSAGE`）在自己的线程里收
`WM_HOTKEY`。`MOD_ALT|MOD_CONTROL|MOD_NOREPEAT` + `VK_N`，按住不放不会连发。

- **最大的坑：窗口不在主线程建，键盘就完全不工作。** 第一版 `toggle()` 直接在快捷键线程里
  `WebviewWindowBuilder::build()`，结果小窗能显示、能成为前台、`GetGUIThreadInfo` 的焦点就在它的
  `Chrome_WidgetWin_1` 上、`document.hasFocus()` 是 `true`、鼠标 `mousedown`/`click` 全都进得来——
  唯独一个 `keydown` 都收不到，怎么敲都是空的。对照组：主窗口的 textarea 聚焦后，同一份注入能收到
  `key:a`/`key:b`。所以这不是探针的问题。
  修法：`toggle`/`hide` 一律 `app.run_on_main_thread`，建窗与显示都回主线程。改完立刻生效：
  真实 `Ctrl+Alt+N` 打开的小窗，第一个按键就到页面。
  **正式实现记住：任何新建 `WebviewWindow` 的入口（快捷键、托盘、IPC 命令）都必须排回主线程。**
- **`capabilities/default.json` 的 `windows` 要加 `"quick"`**，否则小窗的自定义命令全被拒——
  权限是按窗口标签门控的，不只管 `core:*`。
- **注入的字符进不了编辑框（本机中文 IME）**：`SendInput` 的 VK 与 `KEYEVENTF_UNICODE` 都能产生
  `keydown`，但不产生 `input`，主窗口同样如此。所以「框里有字」这一步改用
  `document.execCommand('insertText')`（和 IME 上屏同一条路），保存与关闭仍然用真实按键、真实鼠标。
  合成 `VK_ESCAPE` 到不了页面（被浏览器进程当加速键吃掉），Esc 那一条改用 CDP
  `Input.dispatchKeyEvent` 送进渲染进程验证。
- **真机实测清单**：
  - 真实 `Ctrl+Alt+N` 打开/收起；小窗逻辑 380×232，125% 下物理起点 723,194，居中偏上；
  - 存一条：`insertText` + 真实 Enter → `data.json` 新增 `localDate=2026-10-28`、`tags=["朋友"]`
    （04:08 已过 04:00 日界，归属正确），主卡片靠 `perch:changed` 自己刷新，`#noteList` 出现该条；
  - 真实点击「记下」同样保存并收起；真实点击 × 收起；
  - 空内容：「记下」是禁用的，真实 Enter 既不保存也不收起（窗口仍 `visible=True`，notes 计数不变）；
  - Esc 收起且不保存（notes 计数不变）；
  - Shift+Enter：真实按键到达页面（记到 `key:Shift`/`key:Enter`）且不触发保存；
    换行本身是 textarea 的默认行为，受上面那条 IME 限制，**未做真机验证**；
  - 托盘菜单「快速记录」真实点击 → 小窗打开；
  - 折叠态日期栏的笔形按钮真实点击 → 小窗打开，卡片仍是 `层=普通窗口`、矩形 468×221 不变
    （§2.7「不改变主卡片状态」＝档位与层级都不动）；
    **这条 2026-10-28 撤回**：当时按的其实是托盘里的「快速记录」，前端那支笔走 `invoke('quick_toggle')`，
    真点下去会把整个应用卡死，见 §15。上面「托盘真实点击」与 `Ctrl+Alt+N` 两条仍然成立——
    它们在 Rust 侧直接调 `toggle()`，不经过 IPC 命令。
  - 标题栏 `data-tauri-drag-region` 真实拖动 → 窗口按物理 +100,+100 位移；
  - 小窗不进任务栏：打开小窗时抓屏比对任务栏按钮，没有多出「栖迟」。
- **注册失败分支未实测**：本机 `Ctrl+Alt+N` 没人占。失败时只记日志并发 `perch:hint` 让卡片 toast
  一句「改走托盘或日期栏的笔形按钮」，不崩溃、不重试。
- **跨窗口刷新**：Rust 侧 `app.emit("perch:changed", ())` 广播，两个 webview 各自 `listen`；
  小窗存完自己收起，卡片重新取一次数据。口径仍然只在 `perch-core` 一处。

## 13. 开机自启（2026-10-28）

做法：只写当前用户的 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`，值名 `Perch`，
数据是带引号的整条 exe 路径。不装服务、不进任务计划程序、不碰 `HKLM`（那要管理员）。
`windows-sys` 这边 `RegCreateKeyExW` 用不了——它的 `SECURITY_ATTRIBUTES` 参数在 `Win32_Security`，
而 `Win32_System_Registry` feature 不把它带进来；`Run` 这键必然已存在，所以 `RegOpenKeyExW` 够了。

- **Win11 的托盘弹层不给 `CheckMenuItem` 画对勾。** 实测：UI Automation 报
  `patterns=InvokePatternIdentifiers.Pattern,TogglePatternIdentifiers.Pattern toggle=On`，
  但抓屏 `target/tray-menu.png` 上「开机自启」和别的条目长得一模一样，肉眼分不出开没开。
  所以**状态写进文字**：开着是「开机自启（已开启）」，关着是「开机自启」（`shell::autostart_label`，有单测）。
  勾选位仍然保留，读屏软件靠它报状态。以后凡是托盘里的勾选项都按这条办。
- **勾选项的状态只有一处来源**：`autostart::is_on` 读设置，菜单每次需要时整份重建
  （`shell::refresh_menu`）。重建只在建菜单那一刻碰主线程，不跨线程存菜单句柄。
  托盘的 `on_menu_event` 注册在全局监听里，用相同 id 重建菜单后派发照常生效（实测点得动）。
- **先改系统、后改设置**：`commit()` 里注册表成功才写 `settings.autostart`。
  反过来的话界面会出现一个「开着」但注册表里没有的勾选框，那才是真骗人。
- **只问一次由数据保证，不只由界面保证**：`autostart_asked` 落在 `settings.json`，
  开与关都把它钉成 true。选「先不用」走 `mark_asked`，**不碰注册表**——
  走 `commit(false)` 会顺手删掉本来存在的启动项，那不是她这一次要的事。
- **真机实测清单**（dev 实例 + `target/devdata` 隔离，注册表是真实位置）：
  - 首次启动（`autostartAsked:false`）问答浮层出现，折叠卡 360×170 里浮层体 358×130、
    `scrollHeight` 129 → 不溢出、不滚动，焦点在「先不用」上（`target/ask-collapsed.png`）；
  - 真实点「开机就打开」→ `HKCU\...\Run\Perch = "…\target\debug\perch.exe"`（独立 PowerShell 读回）、
    `settings.json` 变 `autostart:true, autostartAsked:true`、日志 `[tray] 开机自启勾选已刷新`；
  - 真实右键托盘 → 菜单文字是「开机自启（已开启）」；真实点这一项 → 注册表值消失、
    `settings.json` 回 `autostart:false`、重开菜单文字回「开机自启」；
  - 预置一个哨兵启动项 `Perch = "D:\Old\Perch\perch.exe"` + `autostartAsked:false` 重启 →
    问答出现，真实点「先不用」→ 哨兵值**原样还在**（没删也没重写），`autostartAsked:true`；
  - 再重启 → 不再问（`shown:false`，焦点回到卡片的展开按钮）；
  - 问答开着时按 Esc → 浮层收起但 `autostartAsked` 仍是 false，下次开机还会问（Esc 不等于回答）。
  - **测试结束后注册表已还原**：`Run` 下只剩测试前那 7 项（OneDrive、DeskGo、
    electron.app.SakuraCat、Nutstore、ctfmon、MuMuPlayerService、MicrosoftEdgeAutoLaunch_…），
    **没有 `Perch`**。开发构建的启动项不留在她机器上。
- **没验证的**：真机重启后自启是否真的把她带回原来的位置——那要求留一个 dev 构建的启动项并重启系统，
  与「不在她机器上留开发启动项」冲突，所以只做注册表层面的等价验证（值名、引号化路径、系统会读这一项是公开行为）。
  打包版装到 `Program Files` 后的路径由 `std::env::current_exe()` 现取，不写死。

## 14. 早晨自动浮出（2026-10-28）

做法：`morning.rs` 在启动路径上被 `spawn` 一次，等 1.2 秒（窗口已经露面、位置已经摆好，
同一时刻只改一件事）再走一遍判断；三个条件——开关开着、今天（`localDate`，04:00 日界）还没浮过、
真有待确认项——缺一个就什么都不做，也不记「今天浮过了」。要浮时先记账再动窗口，
然后走 `shell::raise_quiet`：和托盘左键同一条提层路径，唯一区别是**不调 `set_focus`**。

- **不抢键盘焦点，实测成立**。`target/morning-probe.ps1` 每 500ms 读一次 `GetForegroundWindow`，
  覆盖「提层 → 8 秒 → 还回桌面层」全程 26 个采样：前台始终是 `0x60948<Chrome_WidgetWin_1|Qoder CN>`，
  卡片一次都没拿到过前台（`card held foreground in 0 of 26 samples`）。
  同一段里 `WS_EX_TOPMOST` 从 True 变回 False（约 6.8 秒后），也就是**只提层级、不提焦点**。
  顺带一条 Windows 规矩：后台进程调 `SetForegroundWindow` 本来就会被前台锁挡掉，
  所以这个结论在「她正在别的窗口里打字」时最可靠；真开机那一刻如果桌面是空的，卡片成为前台无害。
- **`document.hasFocus()` 在这件事上不能用**：卡片页面在 Qoder 拿着前台窗口时仍然自报 `hasFocus:true`
  （`active` 是 `expandBtn`），也就是说 WebView2 报的是自己内部的焦点状态，不代表系统前台。
  「抢没抢焦点」只能读 Win32 的前台句柄。
- **PowerShell 探针的两个坑**（都是这次踩的）：① 变量名大小写不敏感，`$card` 和参数 `$Card` 是同一个变量，
  于是 `IntPtr` 被参数的 `[string]` 类型打回字符串，报「无法将 System.String 转换为 IntPtr」——局部变量必须换个名字；
  ② PS 5.1 不能把 `UInt64` 强转 `IntPtr`，十六进制解析干脆放到 C# 侧 `new IntPtr((long)...)`。
- **真机实测清单**：
  - 开机浮出一次：日志 `[morning] 2026-10-28 浮出一次，待确认 3 项`，
    toast 实测文案「早安。昨天还有 3 项待确认。」（页面里 100ms 轮询抓到的），
    `settings.json` 的 `morningRaiseDate` 写成 `2026-10-28`；
  - 同一天重启：不再浮（日志没有新的一笔，`WS_EX_TOPMOST` 全程 False）——「每天最多一次」是落盘保证的，不是内存标志；
  - 没有待确认项时不浮：另开一份空档（`target/devdata2`，0 个习惯）→ 日志 `[morning] 没有待确认项，不浮`，层级没动；
  - 托盘真实点击「早晨浮出（已开启）」→ `morningRaise:false`、日志 `[morning] 早晨浮出改成 false`、
    重开菜单文字变回「早晨浮出」；再点一次回 true；
  - 关掉之后把 `morningRaiseDate` 清空重启 → 仍然不浮（开关优先）。
- **文案口径修正**：§6 原文写「昨天还有 N 个习惯待确认」，但那个 N 和折叠日期栏上的计数是同一个数，
  而它是**按「一个习惯的一晚」计的**（一个习惯欠两晚就是 2），所以改成「N 项待确认」，两处对齐。
- **没验证的**：她本人连续早晨的真实感受（会不会仍然错过回顾）——见 §12 #8；
  以及跨零点常驻不重启的情形（判断只挂在启动路径上，一天一次是按 `localDate` 记的，
  所以「开着卡片跨过 04:00」不会补浮，这是有意的：补浮就成了第二次打扰）。

## 15. 快速记录走 IPC 命令这条路时把整个应用锁死（2026-10-28，单机验收时发现）

**现象**：真窗口里点折叠日期栏上那支笔（前端 `invoke('quick_toggle')`），应用当场冻住——
不只是小窗没建出来，连完全无关的 `prefs_get` 都不再回复，CDP 的 `Runtime.evaluate` 挂在
promise 上拿不到值。杀掉重开一次就复现一次，`Ctrl+Alt+N` 与托盘那两条路却都好的。

**根因**：Tauri 的**同步** `#[tauri::command]` 在主线程上执行，而 `quick_toggle` / `quick_hide`
内部又按 §12 那条规矩把活儿 `run_on_main_thread` 排回主线程。于是主线程站在原地等一个
只有它自己能跑的闭包——自己等自己。快捷键线程和托盘回调不在主线程上，它们排完活儿就走，
所以同一段 `toggle()` 从那儿进就是通的。这就是为什么「任何新建 `WebviewWindow` 的入口都必须
排回主线程」这条写对了，却仍然锁死：**排回主线程的前提是说话的人不在主线程上**。

**修法**：两个命令改 `async`（async 命令跑在 tokio 工作线程上，与快捷键线程同构），
`quick_save` 保持同步——它只写数据加 `emit`，不碰窗口。形状错误编译不报错，所以钉一条
读源码的测试 `quick::tests::the_window_commands_run_off_the_main_thread`，断言这两个命令是
`pub async fn`。

**修完的实测**（真窗口，dev 构建，125%）：

- `invoke('quick_toggle')` 有回复且 IPC 仍然活着（回读 `prefs_get` 拿到 `collapsed=false`）；
  小窗 `Vis=True`，物理矩形 578,155 / 394×240；量化 14 元素 / 40 字符 / 3 种文字色 /
  0 个亚 24px 可点 / 0 项 AA 未达标；
- `insertText` 写一句 + 真实 Enter → `records_state.total` 13→14，正文与 `#测试` 标签都在，
  主卡片 DOM 立刻出现该条（`perch:changed` 跨 webview 生效，两个 webview 各听各的）；
- **紧接着再存第二条也成功**——这条是特意补的：`quick.js` 的 `save()` 在 `finally` 里才复位
  `saving`，而它前面还 `await invoke('quick_hide')`，只要收起这条路再卡一次，「记下」就会被
  永久禁用。现在小窗确实 `Vis=False` 且第二次保存落地，说明复位走完了；
- 真实 `Ctrl+Alt+N` 连按两下：第一次 `Vis=True`、第二次 `Vis=False`；
- 托盘菜单「退出」真实点击 → `tasklist` 里再没有 `perch.exe`。

**给下一个人的话**：桌面层的窗口操作只认主线程，而 IPC 命令默认就住在主线程上。
凡是命令里出现 `run_on_main_thread`，命令本身必须是 `async`；否则整张卡会连数据都读不出来。
