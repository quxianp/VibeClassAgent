# GUI 完全重构交接记录

> 日期：2026-10-02
>
> 范围：`web/` 完整界面重构，以及原生窗口隐藏/唤醒/退出生命周期修复。

## 1. 回滚保障

重构前已保存两份独立回滚点：

- DSH 手动快照：`20261002-151526-a193`，原因：`VibeClassAgent GUI 完全重构前`。
- 项目本地归档：`_archive/gui-before-redesign-20261002-151542.zip`。

本地归档包含重构前的完整 `web/`、`window.rs`、`server.rs`、`lib.rs` 与 `BASE_COMMIT.txt`。`_archive/` 仅用于本机回滚，不纳入 Git 提交。

## 2. 重构边界

本次不是在旧界面上换配色，而是重新组织了页面壳层、视觉层级与导航结构：

- 新建统一的应用壳层：侧栏、分组导航、顶部工作区、状态区与移动端遮罩。
- 将导航重组为“今日 / 计划 / 连接”，保留全部原页面和接口契约。
- 重写主要 CSS 令牌、布局、卡片、表单、按钮、空状态、弹窗和响应式行为。
- 删除右上角 `default` 身份标识。
- 继续使用纯 HTML/CSS/原生 JavaScript，不引入框架、构建步骤或外部资源。
- `web/index.html` 仍是入口，所有 `/api/*` 路径与请求结构保持不变。

## 3. 资源与生命周期设计

### 3.1 低资源策略

- 不引入 React/Vue、运行时依赖、Web 字体、动画库或远程资源。
- 页面切换不创建第二个 WebView，也不重复注册应用级事件。
- 日志只保留最近 3000 行，避免长时间停留导致数组无限增长。
- 轮询保持低频：日志 1 秒，依赖下载 800 毫秒，仅在对应功能运行时存在。
- CSS 动画数量受控，并支持 `prefers-reduced-motion`。

### 3.2 防泄漏策略

`app.js` 增加页面级生命周期容器：

- `timeout()` / `interval()`：登记定时器。
- `listen()`：登记挂在 `window` 等长生命周期对象上的监听器。
- `cancel()`：停止并从登记集合移除定时器。
- `dispose()`：切页时一次性清理定时器和全局监听器。
- `pageEpoch`：丢弃旧页面晚到的异步结果，防止覆盖新页面或重新绑事件。
- `pagehide`：WebView 真正销毁时释放页面级与应用级资源。
- 依赖下载轮询在完成或弹窗离开 DOM 后主动停止。

普通绑定在页面 DOM 节点上的监听器随旧 DOM 一起回收；拖拽等绑定到 `window` 的监听器通过页面生命周期显式解除。

## 4. 原生窗口生命周期

现在的约定是：

| 入口 | 行为 |
|---|---|
| 系统原生关闭按钮（X） | 只隐藏窗口；WebView、HTTP、后台任务与托盘继续运行 |
| 托盘“打开界面” | 重新显示并聚焦原窗口 |
| 第二次启动 | 命中单实例保护，并按原生窗口标题重新显示同一窗口 |
| GUI“退出程序” | 发送窗口 `Close` 命令，先隐藏、释放 WebView，再退出事件循环 |
| 托盘“退出” | 与 GUI 退出共用相同的显式关闭路径 |

关键修复：

- `window::run()` 在主线程运行 Tao 事件循环，并在进入循环前通过回调发布 `WindowControl`。
- 原生 `CloseRequested` 与程序 `WindowCmd::Close` 分为两种关闭意图，避免系统 X 误走彻底退出。
- `/api/quit` 不再调用 `process::exit()`，避免跳过 WebView、托盘和 Rust 析构。
- 窗口尚未就绪时收到退出请求会登记意图，就绪后补发，不丢失操作。
- Win32 探针按标题获取真实 Tao 顶层窗口；不能使用 `Process.MainWindowHandle`，后者会误选 Tao 的内部事件目标窗口。

Debug 构建提供 `VCA_WINDOW_LIFECYCLE_PROBE=1` 的诊断模式：只跳过受测试机 WebView2 profile 争用影响的 WebView 初始化，Tao 顶层窗口、Win32 `WM_CLOSE`、单实例唤醒、HTTP 退出及产品事件循环均走真实代码；Release 构建中该变量不起作用。

## 5. i18n

新增壳层文案均进入 `locales/zh-CN.json` 与 `locales/en-US.json`，包括导航分组、工作台、主导航、菜单、关闭菜单和本地服务状态。`data-i18n-aria` 用于只在辅助技术中呈现的标签。

品牌名 `VibeClassAgent` 与缩写 `VCA` 属于固定产品标识，不做翻译。

## 6. 验证记录

### 已验证

- 原生 Win32 生命周期探针：
  - 初始窗口可见：`true`
  - 发送 `WM_CLOSE` 后进程存活：`true`
  - 发送 `WM_CLOSE` 后窗口不可见：`true`
  - 第二次启动后同一 HWND 恢复可见：`true`
  - `/api/quit` 后退出码：`0`
- `--no-open` 退出探针：`/api/quit` 返回 `ok=true`，无窗口控制端时正常退出，退出码 `0`。
- Debug 构建：通过。
- `node --check`：外部与内置两份 `app.js` 均通过。
- `zh-CN.json` / `en-US.json`：JSON 解析通过。
- 外部 `web/` 与内置兜底三项资源：SHA-256 完全一致。
- `python scripts/smoke.py`：58/58 通过。
- `scripts/gate.cmd`：fmt、clippy `-D warnings`、workspace tests 三关全过。
- 凭据扫描：本次变更和 Git URL 均未发现 GitHub/OpenAI/AWS/Slack token、私钥或 URL 内嵌凭据。
- 视觉复核：1920×1032 下无明显错位、溢出或文字重叠；右上角不存在 `default` 标识。

### 环境限制（如实记录）

测试机同时存在另一个会自动重启的 WebView2 宿主。直接反复创建生产 WebView 时出现过 `0x800700AA`（profile 正在使用）和 `0x8000FFFF`，因此自动化生命周期探针使用 Debug 专用模式跳过 WebView 初始化。该探针仍使用真实 Tao 顶层窗口、真实 `WM_CLOSE`、单实例分支、HTTP `/api/quit` 与产品事件循环；Release 构建不包含可启用的跳过行为。生产 WebView 分支已通过编译与 clippy，但建议在没有其它 WebView2 profile 争用的交付机上再做一次人工开窗观感确认。
