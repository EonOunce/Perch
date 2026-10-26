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

**方案 A 可行**：卡片能挂进 WorkerW，WebView2 在桌面层里照常渲染、JS 照常跑、子窗口命中链完整、
Explorer 广播后的重挂路径也通。两个必须写进正式实现的前提：**句柄只能缓存一次**、**父窗口被销毁时要能重建窗口**。

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

## 6. 探针自身的坑（留给下一个读代码的人）

- `PrintWindow` 必须带 `PW_RENDERFULLCONTENT`，否则 WebView2 内容区是空的；windows-sys 里这个常量在 `Win32_Storage_Xps` feature 下。
- 手写 BMP 时 `biHeight` 用负数（自上而下）会被一些看图软件解释反（Pillow 直接上下颠倒显示）。
  现在按标准自下而上写行序，看图一致。
- 抓屏（`BitBlt`）读的是最终合成结果，所以"卡片被别的窗口挡住"会照实拍进去，不能当成渲染失败的证据。
