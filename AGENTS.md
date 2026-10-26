# Perch / 栖迟 · 项目约定

Windows 桌面贴层生活记录卡片。`DESIGN.md` 是唯一的需求与设计来源，与代码冲突时以 `DESIGN.md` 为准并回头修正代码。`mockup/` 是一次性交互原型，**不是**正式实现，不要从它复制代码到 `src/` 或 Rust 侧；它的价值只在交互形态与文案。

## 沟通与交付

- 汇报用中文，直说做了什么、验证到什么程度、还有什么没解决。不虚报：没在真机验证过就明确写"未验证"。
- 每个功能点一个 commit，同一 commit 里带上对应测试；测试不过不 commit。
- 构建产物（`.exe`/`.msi`/`.zip`/`target/`）永不进 Git；发布物放 Releases。
- 不修改 `git config`。需要提交身份时用 `git -c user.name=... -c user.email=... commit`。
- 不动与当前任务无关的文件；`data.json`、凭据、日志永不入库（见 `.gitignore`）。

## 不可逆产品决定（改动前必须先问）

1. **同步内容是明文**。"不加密，简单优先"是用户明确做的决定：坚果云 WebDAV 上的记录/日记/打卡为纯文本，只有传输走 HTTPS。不要在 UI、文档或代码注释里出现"已加密""端到端加密"字样；也不要悄悄加客户端加密。坚果云应用专用密码只在本机保护（Windows DPAPI / Android Keystore），不参与同步、不写日志、不导出。
2. **不惩罚**。没有红叉、没有失败色、没有连续中断的羞辱文案、没有扣分、没有强制补签。未确认是中性信息，不是待办失败。文案基调是低负担、可原谅、完全可编辑。
3. **个人默认值一律可改**。目标、单位、周期、提醒、日界时刻都由用户自己定，代码里不写死"应该达到几个"。新增习惯的表单默认值见 `DESIGN.md` §2.4，是可一眼改掉的起点而非建议值。
4. **不预置任何数据**。首次启动是空的：没有示例习惯、没有示例标记、没有示例碎碎念或心情。空状态要有可用入口（「添加第一个习惯」），但不能变成引导动画或打卡催促。
5. **桌面层不是置顶**。卡片属于桌面层，被普通应用正常盖住；托盘左键只是短时提到前台。不要为了实现"永远看得见"改成永久置顶。

## 代码结构约束

- `crates/perch-core` 不 `use tauri`。系统能力（文件路径、DPAPI、窗口、通知）由 shell 侧实现后传入。理由见 `DESIGN.md` §5.4、§5.5：这是手机端能复用同一套口径的前提。
- 口径只有一处来源。日界（04:00 `localDate`）、周归属、完成率、合并规则写在 `perch-core`，前端只渲染，不在 JS 里重算一份。
- 跨语言测试用 `tests/vectors/*.json`：文件里只有输入与期望输出，不含任何平台类型。Rust 与未来的 Kotlin 都读同一组向量。
- 手机端（Phase 4，未开始）走"窄原生"：Kotlin/Compose，**只追加**（记一条、设心情、确认昨晚、当天打卡），不做编辑/删除/归档/统计/标记管理。当前不做 UniFFI。若手机需要支持修改或删除，先把逻辑迁到 UniFFI，再加功能，不要在 Kotlin 里复制一份口径。
- 不引入前端框架或打包器：`src/` 是纯 HTML/CSS/JS。新增依赖要先说明用途、体积、维护状态与替代方案。
- 自动测试不碰真实网络；同步用本地 WebDAV 桩，坚果云真机验证只手工做。

## 验证要求

- **UI 改动必须在真实窗口里看过再交付**（`cargo tauri dev` 或至少 `cargo run` 起的应用窗口）。只看代码、只看类型检查通过，都不算验证过。
- 纯逻辑必须有 Rust 单元测试：日界跨 04:00、周界跨周一凌晨、目标生效日、归档/移除后的过滤、完成率分母、合并与墓碑。
- 桌面层相关改动（嵌入、托盘、快捷键、DPI、Explorer 重启、休眠唤醒）按 `DESIGN.md` §5.2 清单实测，结论写进 `docs/desktop-layer-spike.md`。
- 埋数据风险：涉及读写 `data.json` 的改动，测试必须覆盖"写一半失败不损坏原文件"。

## 命令

```sh
# 环境变量（用户级 PATH 已包含 cargo\bin）
export RUSTUP_HOME="D:\\Installation\\vscode\\WIN development\\rustup"
export CARGO_HOME="D:\\Installation\\vscode\\WIN development\\cargo"

cargo build                      # 整个 workspace
cargo test                       # 全部测试
cargo test -p perch-core         # 只跑纯逻辑
cargo run -p perch               # 起应用窗口（正式验证用）
cargo clippy --all-targets       # 风格与陷阱
cargo fmt --all                  # 提交前格式化
```

数据文件：`%LOCALAPPDATA%\Perch\data.json`。本地设置（窗口位置、折叠状态、同步状态、提醒偏好）单独存，不同步。

## 当前状态

Phase 1（Windows 本地 MVP）。进度看 git log 与 `docs/`；`DESIGN.md` §12 记录仍未定的开放问题，§14 是设计变更记录。
