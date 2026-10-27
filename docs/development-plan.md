# 栖迟 Perch · Windows 本地版落地开发计划（Phase 1 · 新手照着做版）

> 这份计划是什么：它把 `DESIGN.md` 里写好的产品规格，拆成一个**完全没写过 Rust / Tauri 的人也能一步步照做**的施工顺序。每一步都写清「做什么、动哪些文件、怎么知道做对了、对应设计文档哪一节」。
>
> 怎么用：先看「开工前必须先拍板的 4 件事」，拍板后再按里程碑顺序往下做。遇到卡住的地方，先翻文末「回退路径与风险应对」，不要硬磕。
>
> 权威依据：所有口径以 `DESIGN.md` 为准（本计划引用为「设计 §x.y」）。原型在 `mockup/` 下，它只是**视觉与交互的参照**，里面的假数据、假时钟、布景全部不要搬进正式工程。

---

## 0. 名词小词典（第一次出现时怕你看不懂，先统一解释）

| 名词 | 大白话解释 |
|---|---|
| **Rust** | 一种编程语言，比 C++ 安全、比 Python 快。栖迟用它写「后厨」——管数据、管窗口、管文件。 |
| **cargo** | Rust 自带的「包工头 + 包管理器」。编译、跑测试、建工程全靠它，命令长这样：`cargo build`、`cargo test`。 |
| **rustup** | Rust 的安装器。装了它就同时装好了 Rust 编译器和 cargo。 |
| **crate** | Rust 里「一个可复用代码包」的叫法。别人写好的工具库，我们在 `Cargo.toml` 里登记一下就能用。 |
| **Tauri** | 把「网页界面（HTML/CSS/JS）」打包成 Windows 电脑软件的工具。它负责开原生窗口、放托盘、注册快捷键；窗口里显示的内容还是网页。 |
| **WebView2** | Windows 上用来显示网页内容的系统组件（Edge 内核）。你的电脑已装 155.x，不用再装。 |
| **Node.js / npm** | 运行 JS 工具的环境。Tauri 脚手架偶尔会用到；本项目前端**不用任何框架、不打包**，所以装一次就行。 |
| **HWND** | Windows 里每个窗口都有一个编号，叫窗口句柄（Handle to a Window）。后面「把卡片塞进 WorkerW」就是拿这个编号做文章。 |
| **WorkerW** | Windows 桌面壁纸后面那一层隐形窗口。方案 A 的目标：把栖迟卡片挂到这一层里，这样它就贴在壁纸上、被普通窗口正常盖住。 |
| **Win32 API** | Windows 系统给程序员用的那一堆底层函数（找窗口、设父窗口、改样式之类）。 |
| **DPAPI** | Windows 自带的「本机加密」工具，Phase 2 存坚果云密码时才用，Phase 1 先不管。 |
| **WebDAV** | 一种「把网络硬盘当文件夹用」的协议，Phase 2 接坚果云才用。 |
| **UUID** | 一串随机唯一编号（如 `hbt_reading`、`note_71b0`），用来给每条数据一个终身不变的身份证。 |
| **localDate** | 栖迟自己定义的「业务日期」：**一天从凌晨 4 点开始，不是 0 点**。凌晨 2:30 做的事仍算前一天。 |
| **commit** | Git 里「存一次档」的动作。一次只做一件事、存一次档。 |

---

## 1. 开工前必须先拍板的 4 件事（阻塞关键路径）

这 4 件来自设计 §12，**不定下来就没法开始或会返工**。每条给了推荐答案，你点头/改一下就行。

| # | 决策项 | 推荐答案 | 理由 | 状态 |
|---|---|---|---|---|
| 1 | **Tauri 应用标识符（identifier）** | 建议先用 `com.perch.app`（反向域名格式：公司/个人域名倒着写，再加应用名）。如果你以后有自己的域名，比如 `qichi.example.com`，就用 `com.example.qichi`。 | 标识符会写进安装包和系统注册表，**一旦发布很难改**；而且必须在 Tauri 脚手架生成工程**之前**填好（设计 §12 第 2 条）。 | **需用户拍板** |
| 2 | **开机自启默认值** | 推荐：**默认关，第一次启动时弹一个开关让用户自己选**，不偷偷写系统启动项。 | 设计 §5.3 / §12 第 6 条明确「不静默写系统启动项」；栖迟的产品性格是「安静地存在」，不该一上来就自我推销。 | **需用户拍板** |
| 3 | **阅读的默认周目标** | 推荐：**第一周先 `recordOnly`（只记录、不定目标），一周后再让用户自己把周目标改成 5**；或者干脆首版直接 `weeklyTarget = 5`（和 mockup 一致）。 | 设计 §12 第 4 条：需求建议每周 5 晚，但担心新手第一周就有「要做满 5 晚」的压力。从产品原则「低负担、不惩罚」出发，我更倾向「先记录一周」。 | **需用户拍板** |
| 4 | **深色模式做不做（首版）** | 推荐：**V1 只做温暖浅色（纸感），不做深色模式**。 | 设计 §12 第 3 条 + mockup 里根本没有深色模式实现；首版把浅色做扎实，深色以后再加（CSS 变量已经留好了，加起来不难）。 | **需用户拍板** |

> 其余开放问题（§12 第 5、7–18 条等）**不阻塞 Phase 1**，列在文末「延后决策清单」里，做到那一步再定。

---

## 2. 环境准备（照着装，装完验证）

> 你的系统：Windows 10/11 x64，已装 VS Code，WebView2 已是 155.x（满足要求，不用动）。

### 2.1 装 Rust（rustup）

1. 打开 https://rustup.rs/ ，下载 `rustup-init.exe`，双击运行。
2. 出现命令行安装界面时，**直接按回车选默认选项**（`1) Proceed with standard installation`）。
3. 装完**关掉所有终端窗口再重新开一个**（让环境变量生效）。
4. **验证**：新开 PowerShell，输入下面命令，能看到版本号就成：
   ```powershell
   rustc --version
   cargo --version
   ```
   预期输出类似 `rustc 1.xx.x`、`cargo 1.xx.x`。

### 2.2 装 Visual Studio C++ 生成工具（Rust 在 Windows 上编译需要它）

1. 打开 https://visualstudio.microsoft.com/zh-hans/downloads/ ，拉到下面找「**生成工具 for Visual Studio**」下载。
2. 安装器里勾选这一个工作负载：**「使用 C++ 的桌面开发」**（Desktop development with C++）。其余默认。
3. 点安装，等它下完（个头较大，几个 GB）。
4. **验证**：重开 PowerShell，下面命令能跑通不报「linker not found」就成：
   ```powershell
   cargo new hello-rust --bin
   cd hello-rust
   cargo run
   ```
   预期屏幕打印 `Hello, world!`。验证完把 `hello-rust` 文件夹删掉，别留在 Perch 目录里。

### 2.3 确认 WebView2

你的机器已装 155.x，跳过。以后打包给别人时再考虑分发（Phase 5 的事）。

### 2.4 装 Node.js LTS（脚手架选 Vanilla 前端模板时需要）

> 我们的前端是纯 HTML/CSS/JS，但 Tauri 官方脚手架的 Vanilla 模板会带一个 `package.json`，需要用 Node 装一下模板自己的小东西。Node.js 是运行 JS 工具的环境（解释：相当于给电脑装一个「能跑 JS 脚本」的底座）。

1. 打开 https://nodejs.org/ ，下载 **LTS（长期支持版）** 安装包，双击一路下一步。
2. 装完**关掉所有终端再重开一个 PowerShell**。
3. **验证**：
   ```powershell
   node -v
   npm -v
   ```
   预期分别打印版本号（如 `v20.x.x`、`10.x.x`）。

### 2.5 VS Code 建议插件（左侧扩展面板搜名字装）

| 插件名 | 作用 |
|---|---|
| **rust-analyzer** | Rust 代码补全、报错提示（必装） |
| **Tauri**（by Tauri Programs） | Tauri 专用命令和工程识别 |
| **CodeLLDB** | Rust 调试器（可选，先不装也行） |
| 可选：ESLint / Prettier | 前端 JS/HTML 格式化（本项目无框架，可装可不装） |

### 2.6 用 Tauri 脚手架初始化工程（**identifier 必须在这一步前拍板**）

> 前置：先确认第 1 节的「应用标识符」已经拍板（设计 §12 第 2 条）。
>
> 为什么要走「先生成临时目录再搬进来」这一套：Windows 的文件夹名**不区分大小写**，`Perch` 和 `perch` 是同一个目录。如果直接让脚手架生成 `perch` 文件夹，会和你已有的 `D:\Installation\vscode\Perch` 撞车（要么报「目录已存在」，要么把模板文件混进现有目录）。所以我们先生成到一个名字不冲突的临时目录 `perch-temp`，再把模板文件搬进已有的 `Perch` 根目录，你原来的 `DESIGN.md`、`mockup\`、`daybook\`、`docs\` 原地不动。

1. **装两个独立的命令行工具**（注意：是两个不同的工具，别混）：
   - `create-tauri-app`：负责「生成工程模板」。
   - `tauri-cli`：负责后面日常的 `cargo tauri dev` / `cargo tauri build`。
   ```powershell
   cargo install create-tauri-app --locked
   cargo install tauri-cli --version "^2.0.0" --locked
   ```
   > 说明：旧文档里只写 `cargo install tauri-cli` 是**不够的**——它不带 `create-tauri-app` 命令；反过来 `create-tauri-app` 也不带 `dev/build`。两个都要装。

2. 切到上级目录，生成临时工程：
   ```powershell
   cd D:\Installation\vscode\
   cargo create-tauri-app
   ```

3. **脚手架问答按下面选**（对照官方 create-project 文档）：
   - `Project name`：填 **`perch-temp`**（临时名，后面会删掉这个文件夹）。
   - `Identifier`：填你在第 1 节拍板的值，例如 `com.perch.app`。万一还没拍板，先随便填 `com.perch.temp`，搬完后再改 `tauri.conf.json`（下一步）。
   - `Choose which language to use for your frontend`：选 **`TypeScript / JavaScript`**（我们要纯网页前端，不要 Rust 写界面）。
   - `Choose your package manager`：选 **`npm`**（Node 自带，不用额外装 pnpm）。
   - `Choose your UI template`：选 **`Vanilla`**（纯 HTML/CSS/JS，无框架）。
   - `Choose your UI flavor`：选 **`JavaScript`**（不要 TypeScript，我们全用 JS）。

4. **把临时目录里的模板文件搬进 `D:\Installation\vscode\Perch\` 根目录**（在 PowerShell 里跑）：
   ```powershell
   cd D:\Installation\vscode\
   # 把 perch-temp 里除隐藏文件外的全部内容移到 Perch 根目录
   robocopy perch-temp Perch /E /MOVE
   ```
   说明：`robocopy` 是 Windows 自带的文件搬运命令；`/E` 连子文件夹一起搬、`/MOVE` 搬完删除源文件。注意：`/MOVE` 只会删掉源目录里的文件和子目录，**顶层 `perch-temp` 文件夹本身通常会剩一个空壳**，不会自动消失，所以要再手动删一次：
   ```powershell
   Remove-Item perch-temp
   ```
   - 搬完后 `D:\Installation\vscode\Perch\` 根目录应该是这样（对照检查）：
     ```
     Perch\
       DESIGN.md        ← 原有，没动
       mockup\          ← 原有，没动
       daybook\         ← 原有，没动
       docs\            ← 原有，没动
       src-tauri\       ← 脚手架搬来的
       src\             ← 脚手架搬来的（前端目录，后面从 mockup 移植）
       package.json     ← 脚手架搬来的
       index.html / package.json / .gitignore 等模板文件
     ```
   - **不要**移动、重命名 `DESIGN.md`、`mockup\`、`daybook\`、`docs\` 里的任何东西。

5. **确认 identifier**：用 VS Code 打开 `D:\Installation\vscode\Perch\src-tauri\tauri.conf.json`，找到 `identifier` 字段，确认它就是你拍板的值（例如 `"com.perch.app"`）。这是整个工程的身份证，发布后很难改。

6. **装前端模板的依赖并跑起来**（在 Perch 根目录里）：
   ```powershell
   cd D:\Installation\vscode\Perch
   npm install
   cargo tauri dev
   ```
   预期：编译一会儿后弹出一个 Tauri 示例窗口（里面写着欢迎页之类）。**看到窗口 = 脚手架成功**。关掉窗口。

> ⚠️ 这一步只是验证脚手架，**我们不在这里写业务代码**。按本计划里程碑，下一步先做 WorkerW spike（M1）。

---

## 3. 整体里程碑与新手节奏预估

按设计 §9 的顺序，8 个步骤收成 7 个里程碑。时间是给新手的粗估（每天能投入 2–3 小时算）：

| 里程碑 | 对应 §9 步骤 | 干什么 | 粗估时间 |
|---|---|---|---|
| M1 · WorkerW 桌面层 spike | 步骤 1 | 先验证「卡片能不能贴到壁纸那层」 | 3–5 天（失败就切方案 B，不拖） |
| M2 · 项目初始化 | 步骤 2 | git、目录、README、测试基线 | 0.5 天 |
| M3 · 存储层 | 步骤 3 | Rust 读写 data.json、04:00 日界 | 4–6 天 |
| M4 · 今天视图 | 步骤 4 | 日期头、待确认、习惯卡、心情、碎碎念 | 5–7 天 |
| M5 · 记录与统计 | 步骤 5 | 时间线、搜索、热力图、周回顾 | 6–8 天 |
| M6 · 习惯管理 | 步骤 6 | 增改、归档、移除、目标历史 | 4–5 天 |
| M7 · 窗口外壳 Shell | 步骤 7 | 无边框、拖动、折叠、托盘、快捷键、自启、早安浮出 | 5–7 天 |
| M8 · 整体验收 | 步骤 8 | 断网可用、重启保留、测试全绿 | 1–2 天 |

**总计大约 4–6 周**（新手放慢点按 6–8 周规划）。

---

## 4. 细任务清单（按里程碑，每个任务四要素：做什么 / 动哪些文件 / 怎么验证 / 对应设计节）

> 贯穿全程的规矩（设计 §10）：**一个功能点 = 改文件 → 实现 → 写/跑测试 → 手动点一遍 → 一次 commit**。

### M1 · WorkerW spike（设计 §5.2、§9 步骤 1）

> 这是整个项目第一件事。先别写业务，只验证一件事：「WebView2 透明窗口能不能稳稳挂在 WorkerW 上」。

- **任务 1.1 起一个最小 Tauri 窗口**
  - 做什么：在脚手架基础上，把默认窗口改成无边框、透明、360×480，放一个红色方块当占位。
  - 文件：`src-tauri/tauri.conf.json`（窗口配置）、`src/index.html`、`src/styles.css`。
  - 验证：`cargo tauri dev`，看到一个透明底、带红方块、没有标题栏的小窗。
  - 对应：§2.1（无边框、圆角、透明）、§5.1。
- **任务 1.2 写 WorkerW 嵌入代码**
  - 做什么：在 Rust 里调用 Win32 API：`FindWindow("Progman")` → 发消息 `0x052C` → 枚举 WorkerW → 找到带 `SHELLDLL_DefView` 的那个 → 把自己的 HWND `SetParent` 上去。退出时 `SetParent(hwnd, null)` 还原。
  - 文件：`src-tauri/src/desktop_layer.rs`（新建）、`src-tauri/src/main.rs`（启动时调一次）。
  - 验证：`cargo tauri dev`，红方块**出现在壁纸上、图标后面**；用别的窗口盖住它，它被正常遮住；按 `Win+D` 看它还在不在；重启资源管理器（任务管理器里重启 explorer.exe）后它会不会丢（丢了就要监听 `TaskbarCreated` 消息重嵌）。
  - 对应：§5.2 方案 A 4 步 + 实测清单。
- **任务 1.3 把结论写下来**
  - 做什么：新建 `docs/desktop-layer-spike.md`，记录：Win10/Win11 下 WorkerW 成不成功、`Win+D`/Explorer 重启/多屏/DPI 各场景表现。
  - 文件：`docs/desktop-layer-spike.md`。
  - 验证：这个文件能回答「方案 A 行不行」。
  - 对应：§5.2 成功标准。
- **失败就切方案 B**（见第 8 节回退路径），不阻塞 M2 以后。

### M2 · 项目初始化（设计 §9 步骤 2、§10）

- **任务 2.1 git 仓库**
  - 做什么：`git init`，写 `.gitignore`（忽略 `target/`、`node_modules/`、`*.log`、凭据文件等）。
  - 文件：`.gitignore`、根目录。
  - 验证：`git status` 干净；`git log` 能看到第一个 commit。
  - 对应：§9 步骤 2。
- **任务 2.2 README 与 AGENTS.md**
  - 做什么：README 写「怎么跑（`cargo tauri dev`）、怎么测（`cargo test`）、目录说明」；`AGENTS.md` 写团队规矩（**一次改动一个 commit、一次改动同步测试**）。
  - 文件：`README.md`、`AGENTS.md`。
  - 验证：新人照 README 能跑起来。
  - 对应：§9 步骤 2、§10。
- **任务 2.3 Rust 格式化与测试基线**
  - 做什么：跑一次 `cargo fmt` 和 `cargo test`，确认空工程也能编译、测试能跑。
  - 文件：无（命令验证）。
  - 验证：`cargo test` 全绿（空测试也算绿）。
  - 对应：§10。

### M3 · 存储层（设计 §3、§5.4、§9 步骤 3）

> 这一里程碑**不写界面**，只写「后厨」并配单元测试。

- **任务 3.1 定 Rust 数据结构**
  - 做什么：照设计 §3.2 的六组集合（`habits` / `markers` / `checkinEvents` / `notes` / `dailyMoods` / `tombstones`）写 Rust struct。字段名跟 JSON 一字不差。
  - 文件：`src-tauri/src/storage.rs`。
  - 验证：`cargo build` 通过。
  - 对应：§3.2、§3.3。
- **任务 3.2 写 04:00 日界模块**
  - 做什么：写 `local_date(now) = (本地时间 - 4 小时) 的日历日期`；配套「昨天」「本周周一」「下周周一」「按 localDate 归周」函数。
  - 文件：`src-tauri/src/date_boundary.rs`。
  - 验证：**单元测试**覆盖：凌晨 02:30 算前一天、04:00 整算新一天、周一早晨确认周日晚→归上周。命令 `cargo test date_boundary`。
  - 对应：§2.3、§3.3 日界约定、§5.4。
- **任务 3.3 写文件读写（原子写 + 备份）**
  - 做什么：把数据存到 `%APPDATA%\Perch\data.json`；先写临时文件再改名（原子写）；写之前在 `backups\` 留一份滚动备份（保留 14 份）。
  - 文件：`src-tauri/src/storage.rs`、`src-tauri/src/commands.rs`。
  - 验证：手动改 data.json 让它损坏 → 重启应用能从 backups 恢复；`cargo test storage` 覆盖原子写逻辑。
  - 对应：§3.1。
- **任务 3.4 goalHistory 生效规则**
  - 做什么：「改已有习惯的周目标 → 从下一个周一生效；新增习惯 → 从本周周一生效；生效日晚于今天 = 待生效」。
  - 文件：`src-tauri/src/date_boundary.rs`（算周一）+ `storage.rs`（写 goalHistory）。
  - 验证：单元测试喂一组日期，断言「本周仍按旧目标、下周起按新目标」。
  - 对应：§2.4 目标历史、§3.2 habit.goalHistory。
- **任务 3.5 把「读全量数据」暴露成 Tauri 命令**
  - 做什么：写 `load_all()` 命令，前端启动时调一次拿到全部数据（先不做增量，全量简单）。
  - 文件：`src-tauri/src/commands.rs`。
  - 验证：`cargo build` 通过；后面 M4 前端能调通。
  - 对应：§5.4 commands。

### M4 · 今天视图（设计 §2.1–§2.7、§9 步骤 4）

> 从这一步开始，前端 `src/` 开始写。**所有数据通过 Tauri 命令拿，前端不直接读文件**。

- **任务 4.1 页面骨架与三标签**
  - 做什么：把 mockup 的 `index.html` / `styles.css` 骨架搬过来：顶栏、三个 tab（今天/记录/统计）、卡片容器。布景（壁纸、模拟窗口、任务栏）**全删**。
  - 文件：`src/index.html`、`src/styles.css`。
  - 验证：`cargo tauri dev`，看到栖迟卡片白底、三个 tab 能点。
  - 对应：§2.1、§2.2。
- **任务 4.2 日期头**
  - 做什么：前端调 `get_today`，后端返回今天的 localDate、星期、第几周，前端渲染「周四 · 第 43 周 / 10月22日」。
  - 文件：`src/app.js`（渲染）、`src-tauri/src/commands.rs`（`get_today`）。
  - 验证：改系统时间到凌晨 03:59 / 04:01，日期头显示不同日期。
  - 对应：§2.3 日界、§2.1。
- **任务 4.3 待确认区（次日确认习惯）**
  - 做什么：后端算「昨晚有哪些 nextDay 习惯没打卡」，逐行返回；前端每行显示「完成 / 没做」+ 区域「稍后」。点完成/没做 → 调 `resolve_review`，后端追加一条打卡事件（不覆盖旧的）。
  - 文件：`src/app.js`、`src-tauri/src/commands.rs`、`src-tauri/src/storage.rs`。
  - 验证：凌晨 4 点后，昨天是 nextDay 习惯且没记录 → 今天看到待确认行；点完成 → 写入 data.json 的 `checkinEvents`；再点改成没做 → 追加一条更正事件，旧的不删。
  - 对应：§2.3。
- **任务 4.4 多习惯卡（当天打卡 + 次日待确认）**
  - 做什么：`sameDay` 习惯显示「打卡」按钮，点了写 `checkin` 事件、卡片变「已打卡 + 撤销」；`nextDay` 习惯当晚**不显示打卡按钮**，只显示状态。
  - 文件：同上。
  - 验证：两个预置习惯（阅读=nextDay、散步=sameDay）行为符合；撤销=写一条 `void` 事件。
  - 对应：§2.4、§3.2 status: done/notDone/void。
- **任务 4.5 心情滑杆**
  - 做什么：0–100 滑杆，拖动中实时预览（不改后端），松手才 `save_mood`；一天一条，再拖覆盖。
  - 文件：`src/app.js`、`src/styles.css`、`commands.rs`。
  - 验证：拖滑杆数字实时变、不弹确认；松手后重开应用值还在；一天只有一条。
  - 对应：§2.6。
- **任务 4.6 快速碎碎念（内联）**
  - 做什么：输入框写文字、`#标签` 自动解析、Ctrl+Enter 保存 → `add_note`；下方显示今日记录列表。
  - 文件：`src/app.js`、`commands.rs`。
  - 验证：写一条带 `#阅读` 的碎碎念，data.json 里 `notes[]` 多一条、`tags` 是 `["阅读"]`（不带 #）。
  - 对应：§2.5、§2.7。

### M5 · 记录与统计（设计 §2.5、§6、§9 步骤 5）

- **任务 5.1 记录时间线**
  - 做什么：调 `list_notes`，按 localDate 倒序分组渲染；搜索框、标签筛选 chip；编辑（只改 text/tags，不动时间）、删除（写墓碑 + 10 秒撤销）。
  - 文件：`src/app.js`、`commands.rs`（`list_notes`/`update_note`/`delete_note`）。
  - 验证：搜关键词能过滤；删一条 → 列表消失、toast 撤销能恢复；data.json 里是 tombstone 而不是真删。
  - 对应：§2.5、§3.3 墓碑约定。
- **任务 5.2 统计控制区**
  - 做什么：习惯范围（全部/单个）、时间范围（7/30/90/本月/上月/自定义），切范围 → 调 `get_stats`。
  - 文件：`src/app.js`、`commands.rs`（`get_stats`）。
  - 验证：切范围，首屏数字和热力图一起变。
  - 对应：§6.1、§6.2。
- **任务 5.3 完成率与热力图**
  - 做什么：后端算「完成率 = done / (done + notDone + 未确认)」，分母只算到昨天；前端画单色相热力图（l2 达标 / l1 部分 / l0 没做 / pending 描边 / 未来完全透明）。
  - 文件：`src-tauri/src/stats.rs`、`src/app.js`。
  - 验证：单元测试 `cargo test stats`：分母为 0 显示「—」不显示 0%；未来日期不上色；未确认留在分母。
  - 对应：§6.2。
- **任务 5.4 记录量 / 标签 / 心情趋势 / 上周回顾**
  - 做什么：记录量柱状、Top5 标签、心情折线（少于 2 个点不画线）、上周文字回顾（规则生成 Markdown）。
  - 文件：`stats.rs`、`commands.rs`（`export_week_review`）、`src/app.js`。
  - 验证：心情只记了 1 天 → 趋势位显示「数据不足」；导出上周回顾 → 拿到一个 .md 文件内容。
  - 对应：§6.3、§6.4、§6.5。

### M6 · 习惯管理（设计 §2.4、§9 步骤 6）

- **任务 6.1 管理面板**
  - 做什么：「习惯」区块标题右侧「管理」打开面板；列表/新增表单/确认三态互切。
  - 文件：`src/app.js`、`commands.rs`（`list_habits`/`save_habit`）。
  - 验证：能打开、能看到在列习惯和归档区。
  - 对应：§2.4。
- **任务 6.2 新增 / 编辑习惯**
  - 做什么：表单填名称/单位/确认时机/周目标/if–then；保存时后端自动分配图标与配色（sage/clay/amber/mist 循环）；目标变了 → goalHistory 追加一条、`effectiveFrom` = 下周一。
  - 文件：`commands.rs`、`storage.rs`。
  - 验证：改周目标后，本周仍按旧目标显示，下周一起按新目标；界面提示「X 月 X 日起每周 N 晚」。
  - 对应：§2.4、§3.2。
- **任务 6.3 归档 / 恢复 / 统计开关**
  - 做什么：归档写 `archivedAt`，习惯从今天视图消失、进管理面板归档区；带 `statsVisible` 开关（默认开）；恢复则 `archivedAt = null`。
  - 文件：`commands.rs`（`archive_habit`/`restore_habit`/`set_habit_stats_visible`）。
  - 验证：归档后今天页看不到它，但统计里仍有它（开关开时）；关掉开关 → 统计里也消失。
  - 对应：§2.4 归档语义、§6.2。
- **任务 6.4 移除（墓碑）**
  - 做什么：一次就地确认 → 写 `tombstones`（entityType=habit）；不给撤销窗口。
  - 文件：`commands.rs`（`remove_habit`）。
  - 验证：移除后，今天/习惯范围/长条/统计首屏/热力图**全部**看不到它；data.json 里留着 tombstone。
  - 对应：§2.4 移除语义、§3.3。

### M7 · 窗口外壳 Shell（设计 §2.1、§2.8、§5.3、§9 步骤 7）

- **任务 7.1 无边框 + 拖动**
  - 做什么：顶栏空白处拖动（4px 阈值，超过才算拖）；窗口位置存设置、重启还原。
  - 文件：`src/app.js`（拖动逻辑）、`src-tauri/src/commands.rs`（`get_prefs`/`set_pref`）。
  - 验证：拖到哪儿重启还在哪儿；点品牌块不触发拖动。
  - 对应：§2.1 拖动热区。
- **任务 7.2 折叠态**
  - 做什么：点品牌块 → 窗口 resize 到 360×169，露出横向日期栏 + 竖向长条；长条里已打卡行裁成窄条。
  - 文件：`src/app.js`、`src/styles.css`、`src-tauri`（resize 命令）。
  - 验证：展开/折叠是同一个点；长条收展状态记忆。
  - 对应：§2.8。
- **任务 7.3 系统托盘**
  - 做什么：托盘图标常驻；左键临时提到前台；右键菜单：显示/隐藏、开机自启、退出。
  - 文件：`src-tauri/src/main.rs`、`tray` 相关 Rust 代码。
  - 验证：右键菜单每一项都有用；退出后托盘图标消失。
  - 对应：§5.3。
- **任务 7.4 全局快捷键 + 快速记录窗**
  - 做什么：注册 `Ctrl+Alt+N` 开一个独立置顶小窗；Enter 保存、Shift+Enter 换行、Esc 关；注册失败 → 不崩，托盘里仍有入口。
  - 文件：`src-tauri`（快捷键 + 第二窗口）、`src/app.js`。
  - 验证：全局按 `Ctrl+Alt+N` 弹出小窗；保存后主卡片列表多一条。
  - 对应：§2.7、§5.3。
- **任务 7.5 开机自启开关 + 早安浮出**
  - 做什么：设置里一个开关（默认按第 1 节拍板结果）；04:00 后有待确认习惯 → 卡片短暂浮出一次（不抢键盘焦点）。
  - 文件：`src-tauri/src/main.rs`、`commands.rs`。
  - 验证：开关切换后，重启电脑自启状态跟着变；一天只浮出一次。
  - 对应：§5.3、§2.3。

### M8 · 整体验收（设计 §9 步骤 8、§13）

- 断网开应用 → 全部功能能用（本地优先）。
- 重启电脑 → 数据、窗口位置、折叠状态都在。
- 全界面找一遍：没有红叉、没有「连续中断」羞辱文案、没有扣分。
- `cargo test` 全绿；核心路径手动点一遍（待确认→打卡→写碎碎念→看统计→管理习惯→折叠→快速记录→托盘退出）。
- 对照 mockup §13 的量化基线（今天视图可见元素 ≤120、文字色 2–4 种、AA 0 项）自测。

---

## 5. 从 mockup 到真实工程：后厨/前厅迁移专章

### 5.1 硬边界（死规矩）

> **前端（HTML/CSS/JS）永远不许直接碰文件系统。** 读数据、写数据、算日期、算统计，全部通过 Tauri 命令喊 Rust 去做。前端只做「把 Rust 给的数据画出来 + 把用户操作打包成命令发回去」。
>
> 这么做的原因：以后 Phase 4 要做安卓版（Kotlin 原生），后厨那套 Rust 逻辑可以直接照搬思路；前端换皮就行，不用重写算法。

### 5.2 哪些逻辑搬到 Rust（后厨）

| 类别 | 内容 | 对应设计节 |
|---|---|---|
| 文件存储 | data.json / settings.json 读写、原子写、备份、迁移 | §3.1、§5.4 storage.rs |
| 日界与日期 | 04:00 分界、localDate、周一/周日归属、isoWeek | §2.3、§3.3、§5.4 date_boundary.rs |
| 事件归并 | 同习惯同晚多条事件取最新、done/notDone/void 状态判定 | §3.3 |
| 目标历史 | goalHistory 生效周判定、待生效 | §2.4、§3.2 |
| 统计计算 | 完成率公式、连续达标周、热力图格子等级、记录量、心情趋势、上周回顾文本 | §6 全部、§5.4 stats.rs |
| 习惯过滤 | 在列 / 归档 / 进统计 / 被墓碑过滤 四种视角 | §6.2 |
| ID 生成 | UUID v4 + 类型前缀，后端统一发 | §3.3 |
| 未来扩展 | 同步（WebDAV / 合并 / 凭据）放 `sync/` 目录，Phase 2 才写 | §4、§5.4 |

### 5.3 哪些留在前端（前厅）

| 类别 | 内容 |
|---|---|
| 渲染 | 所有 `render*` 函数、HTML 模板、手写 SVG 折线/热力图（可从 mockup 原样搬） |
| 事件 | 按钮点击、tab 切换、表单输入、拖动、Esc 关弹窗 |
| 动画/视觉 | ≤250ms 过渡、折叠裁窄动画、toast 堆叠、滚动淡出 |
| 纯展示计算 | 心情滑杆拖动中的预览（不落库前不算后端）、标签输入预览 |
| 窗口形态 | 折叠/展开 resize、4px 拖动阈值、位置夹取（调 Rust 的窗口命令完成） |
| 安全 | HTML 转义 `escapeHtml`、`#标签`富文本渲染——**这条必须留前端** |

### 5.4 Rust 核心模块边界（照 §5.4）

```
src-tauri/src/
  main.rs            # 入口：开窗口、贴桌面层、挂托盘、注册快捷键
  desktop_layer.rs   # WorkerW 嵌入 / 方案 B 回退（M1 写）
  storage.rs         # data.json 读写、原子写、备份、数据模型（M3 写）
  date_boundary.rs   # 04:00 日界、周归属、goalHistory 生效日（M3 写）
  stats.rs           # §6 全部统计口径（M5 写）
  commands.rs        # 前端 ↔ Rust 的命令层（每个里程碑都加）
  sync/              # Phase 2 才建：webdav.rs / merge.rs / credential.rs
```

### 5.5 前端 ↔ Rust 命令/事件往返总表

> 命令命名风格：`snake_case`。前端用 `invoke("命令名", { 参数 })` 调；Rust 用 `emit("事件名", 数据)` 推。

**命令（前端 → Rust）**

| 命令 | 入参 | 返回 | 干什么 |
|---|---|---|---|
| `load_all` | — | 全量数据对象 | 启动时拿一次全量（M3） |
| `get_today` | — | `{ date, weekday, pendingReview[], habits[], todayNotes[], todayMood }` | 今天视图要的一切（M4） |
| `resolve_review` | `{ habitId, status: "done"\|"notDone" }` | 新事件 | 点「完成/没做」（M4） |
| `checkin` | `{ habitId, status, localDate?, source }` | 新事件 | 当天打卡 / 撤销（M4） |
| `save_mood` | `{ score: 0–100 }` | 落库后的心情记录 | 松手保存心情（M4） |
| `add_note` | `{ text }` | `{ id, localDate, tags }` | 写碎碎念（M4/M5） |
| `update_note` | `{ id, text }` | 更新结果 | 编辑记录（保留原时间） |
| `delete_note` | `{ id }` | — | 软删（写墓碑） |
| `list_notes` | `{ query?, tag? }` | 分组后的记录数组 | 记录时间线 |
| `list_habits` | — | `{ active[], archived[] }` | 习惯管理面板 |
| `save_habit` | `{ mode, id?, draft{} }` | 校验错误或结果 | 新增/编辑习惯 |
| `archive_habit` / `restore_habit` / `set_habit_stats_visible` / `remove_habit` | `{ habitId }` 等 | — | 习惯四个写操作 |
| `add_marker` / `delete_marker` | `{ habitId, localDate, label }` / `{ markerId }` | — | 标记（M5/M6） |
| `get_stats` | `{ range, habitId, start?, end? }` | `{ spec, totals, perHabit, heatCells, volumeBuckets, moods, topTags, timeBuckets, weekReview }` | 统计页一屏数据 |
| `export_week_review` | `{ habitId }` | Markdown 字符串 | 上周回顾导出 |
| `get_prefs` / `set_pref` | key / key+value | — | 窗口位置、折叠状态等本地设置 |

**事件（Rust → 前端推送）**

| 事件 | 何时推 | 前端做什么 |
|---|---|---|
| `day_changed` | 跨过 04:00 日界 | 重新调 `get_today` 刷新界面（不然开着卡片过午夜还是昨天的样子） |
| `review_due` | 04:00 后发现有待确认习惯 | 触发早安浮出 / 系统通知（替代 mockup 里的 localStorage 去重） |
| `sync_changed` | （Phase 2 预留）同步完成 | 重新 `load_all` |

---

## 6. 测试与提交约定（设计 §10）

### 6.1 一次改动一个 commit

- 规矩：每做完一个任务（就是第 4 节里的一条），先自测 → 再 commit。
- 命令示例（PowerShell）：
  ```powershell
  git add src-tauri/src/date_boundary.rs
  git commit -m "feat: 实现 04:00 日界与周归属"
  ```
- 一次 commit 只做一件事；别把「加日界 + 改心情滑杆」塞同一个 commit。

### 6.2 Rust 单元测试重点区

写在 `src-tauri/src/*.rs` 里的 `#[cfg(test)] mod tests`，或 `tests/` 目录。**新手优先把纯逻辑测了**（不用开窗口、不用网络）：

| 测试什么 | 关键用例 |
|---|---|
| 04:00 日界 | 02:30 → 前一天；04:00 整 → 新一天；03:59 → 前一天 |
| 周归属 | 周一早晨确认周日晚 → 该晚归上周；本周周一到周日怎么算 |
| 目标历史 | 改周目标 → 本周仍旧目标、下周起新目标；新增习惯 → 从本周周一起 |
| 统计口径 | 完成率 = done/(done+notDone+未确认)；分母只算到昨天；分母 0 显示「—」；连续达标周不被进行中的周打断 |
| 事件归并 | 同晚多条事件取最新；void 事件不算 done 也不算 notDone |

跑测试：
```powershell
cargo test
```
预期全绿。某一块单独测：`cargo test date_boundary`。

### 6.3 UI 改动手动验证

- Rust 测试管不到界面。**每改一次前端（`src/`），都要 `cargo tauri dev` 真点一遍**：按钮能不能点、字有没有溢出、折叠/展开顺不顺。
- 浏览器里也能先粗看：`src/index.html` 直接用 VS Code 的 Live Server 打开（但 WorkerW、托盘、文件读写在浏览器里没有，那部分必须回 Tauri 里验）。
- 不引入测试框架之外的大依赖；前端保持纯 HTML/CSS/JS，不装 Vue/React。

---

## 7. 回退路径与风险应对

### 7.1 WorkerW spike 失败 → 方案 B（普通无边框窗口）

- 怎么切：`desktop_layer.rs` 里加一个开关，不做 `SetParent`，直接用 Tauri 默认无边框、不置顶、`skipTaskbar` 的普通窗口，放右下角。
- 不阻塞：方案 B 下，M2 以后所有业务功能照常做，只是卡片在「普通窗口层」而不是「壁纸上」。产品文案不要承诺「永远贴在壁纸」。
- 可诊断：设置里加一行小字显示当前是「桌面层模式」还是「普通窗口模式」，不给用户看技术噪音。
- 触发条件：Win10/Win11 关键场景下 WorkerW 不稳定（透明合成坏、`Win+D` 后消失、Explorer 重启后窗口丢），就直接切 B。

### 7.2 哪些环节即使失败也能后置，不阻塞主线

| 环节 | 失败/做不动时怎么办 |
|---|---|
| 系统托盘 | 先用命令行退出应用，托盘 M7 再补 |
| 全局快捷键 Ctrl+Alt+N | 注册失败就留托盘入口，不崩（§5.3 已规定） |
| 开机自启 | Phase 1 先做开关，默认值按拍板结果；实现细节可后置 |
| 早安自动浮出 | 先只做「有待确认时卡片在右下角提示」，浮出动画后置 |
| 多显示器 / DPI | 先在主屏 100% 缩放下跑通，多屏夹取后置 |

### 7.3 其他主要风险（设计 §11）

- Explorer 重启后父窗口消失 → 监听 `TaskbarCreated` 消息重新嵌入。
- 统计被误解为诊断 → 文案写「相关不等于因果」「样本还少」，不输出「有效/无效」。
- 改目标导致历史被重判 → goalHistory 按生效周算，原始事件不动。
- 名称/商标 → Phase 5 发布前再检索，Phase 1 不投品牌资产。

---

## 8. 延后决策清单（不阻塞 Phase 1，做到那一步再定）

来自设计 §12，这里只列出、不在关键路径上：

- 第 5 条：Perch / 栖迟 商标与商店可用性（发布前再说）。
- 第 7 条：周界周一–周日是否定死（先按周一–周日做）。
- 第 8 条：早安浮出要不要短暂抢焦点（实机体验后定）。
- 第 9 条：每周回顾编辑粒度（先只允许改文字）。
- 第 10 条：同步明文提示位置（Phase 2 再说）。
- 第 11–18 条：长条行数上限、多习惯浮出排序、心情段位名、习惯数量上限、今天视图重心、两套确认动作并存、心情是否在首屏、删除入口常驻——这些做到对应界面时再拍。

---

## 9. 下一步

1. 先回到第 1 节，把 4 件事拍板（尤其 **identifier**，不然后面脚手架白搭）。
2. 按第 2 节装环境（rustup + VS C++ 生成工具）。
3. 从 M1 WorkerW spike 开工；跑不通就切方案 B，别在这一步耗死。
4. 每个里程碑做完，`cargo test` + 手动点一遍 + 一次 commit，再往下走。
