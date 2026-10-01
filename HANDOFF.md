# VibeClassAgent 交接文档（HANDOFF）

> **给接手的 AI / 开发者**：这份文档的目的是让你**在 10 分钟内接上工作**，
> 而不是像第一轮那样从头读一遍代码再猜上下文。
> 里面每一条都基于实际做过的事和实测结果；不确定的地方明确标注了「待确认」。
>
> 最后更新：2026-10-01 ｜ 对应提交：本轮依赖自检 + 单实例 + 托盘修复（未提交，见 §12）

---

## 0. 先读这一节（最重要）

**这个项目已经能跑，不要再从头重构。**

- 代码 **399 个单元测试全绿**（2 个 ignored 的需要外网），
  `cargo clippy --workspace --all-targets -- -D warnings` **零告警**，`cargo fmt --all -- --check` **无差异**。
- 四大核心功能（静默录制 / 课后处理 / 推送 / 清理）**已端到端验证通过**。
- GUI 冒烟 `python scripts\smoke.py` **58 项全过**。
- 上一任 agent 留下的 4.2 GB 垃圾（Python 运行时、工具链、构建产物）已清理，
  现在是纯 Rust，约 190 MB（含 ffmpeg + whisper 模型）。
- 纯 Rust 目标保持住了：依赖自检与下载**没有**外挂 `scripts/fetch-deps.py`，
  下载器就在 `vca-platform/src/fetch.rs` 里。

**接手后第一件事**：跑 `scripts\dev.cmd doctor` 和 `scripts\dev.cmd debug e2e`，
确认真实状态与你读到的一致，再动代码。

**三条硬规矩**（违反会直接破坏用户信任）：

1. **不要在 README / 文档里写没验证过的数字**。上一任 agent 的 README 同一页里写着
   「109 个测试」和「88 个测试」，实测是 123——这就是用户说"太垃圾了"的直接原因之一。
   > 本文件 §8 的「未验证」表是这条规矩的产物：**没验过的功能就写没验过**，
   > 包括本轮新加的「依赖下载」（开发机连不上外网，只验了检测与策略）。
2. **不要提交凭据**。所有 token 走环境变量或 `secrets.env`（已被 .gitignore 排除）。
3. **改动必须过三关再提交**：`cargo fmt --all` → `cargo clippy --all-targets` → `cargo test --workspace`。
   直接跑 `scripts\gate.cmd` 一次做完三关。

---

## 1. 项目是什么

**轻量级静默课堂录制与课后总结系统**，运行在希沃（Seewo）一体机等教学大屏上。

用户的原始需求（第一条消息给的提示词）：

> ① 根据自定义「上课时段」在后台**静默**录屏录音，时段结束自动停止，无提示无弹窗无托盘打扰；
> ② 在「午休期间」自动课后处理：调用用户自填的模型 API 做语音转写 → 提取每节课核心内容 →
>    关联课堂桌面截图 → 生成带图文档并本地保存；
> ③ 通过微信/QQ 机器人把文档推送到用户聊天页面；
> ④ 全部完成后按策略删除本地录屏文件。
>
> 约束：插件架构（各模块可插拔）、提供类似 Claude Code 的 CLI 交互页面、
> 不限第三方库但要保证稳定性。

**用户后续补充的要求**（都记在这份文档里，别再问一遍）：

| 补充 | 落地位置 |
|---|---|
| 转写优先本地，云端可选（云端太烧钱） | `vca-platform/src/stt.rs` |
| 微信/QQ **四个渠道都要** | `vca-platform/src/push.rs`（实际做了六种） |
| API 要**全兼容** | OpenAI 兼容协议为底座 + 10 个厂商预设 |
| 提取支持付费 API / 免费额度 / 免费网页 chatbot 三种模式 | `vca-platform/src/llm.rs` + `browser_bot.rs` |
| **设置不要放 C 盘**，让用户自己选 | `vca-core/src/paths.rs` |
| 打包的 exe 要**自带运行时** | `scripts/package.py` |
| 模型名要**自动拉取** | `llm::list_models()` + `/model` 命令 |
| CLI 固定文案要外置成**语言文件**（便于改词/多语言） | `vca-core/src/i18n.rs` + `locales/*.json` |
| 配色排版还原 **Claude Code**，缩写 **VCA** 用 ASCII 字符画 | `vca-cli/src/repl.rs` |
| 首次引导要**问 API 并自动拉模型** | `vca-cli/src/cmd.rs` 的 setup |
| 任务栏图标（正式 LOGO 稍后给，先用占位图） | `assets/icon.ico` + `vca-platform/src/tray.rs` |
| 保留**带数字序号的便捷引导**，`/` 命令额外列为开发者用 | `repl.rs` 的 `menu_action` |

**测试机配置**（"性能最差的一台"，设计基线按它定）：

```
Intel i5-10400 (6C12T) / 8 GB RAM / 64 位 / 十点触控
```

---

## 2. 现在是什么状态

> **交付形态（2026-09 起）**：现在有**两种形态**，主线是 GUI。
>
> - **GUI（主线）**：`vca gui` 在本机起一个只绑回环的 HTTP 服务，
>   用系统自带的 Edge 以「应用模式」打开一个没有地址栏的独立窗口
>   （有任务栏图标、能最小化）。前端是纯 HTML/CSS/JS，用 `include_str!`
>   打进 exe —— **目标机器上不需要装任何运行时**。
>   URL 里带一次性令牌，接口没有令牌一律 403。
> - **CLI（已存档）**：`vca chat` 仍在、仍能用，但不再往前做，
>   代码冻结在标签 `cli-archive`。日常一律走 GUI。
>
> 代价要知道：**前端资源在 exe 里，改完必须重新编译**（`cargo build`）。
> 想在开发时不重编，可以把改动放到 `<程序目录>/assets/web/` 下，
> `web::resolve()` 会优先用外部文件。另外**直接关掉界面窗口不会退出后台服务**
> ——窗口是 `--app` 拉起的独立进程。要真退出，用界面右上角的「退出」。

```mermaid
flowchart LR
  A["录制<br/>已通"] --> B["转写<br/>已通"]
  B --> C["提取<br/>已通"]
  C --> D["截图关联<br/>已通"]
  D --> E["文档<br/>已通"]
  E --> F["推送<br/>已通"]
  F --> G["清理<br/>已通"]
```

**实测数据**（i5-10400，出自真机运行，不是估算）：

| 项 | 值 |
|---|---|
| 录制产物 | 10.4 秒 720p@8fps → 396 KB（1152×720 h264 + 48 kHz 立体声 aac，305 kb/s） |
| 换算 | 一节课 45 分钟约 **105 MB** |
| 本地转写 | 10 秒音频 → **1.0–1.4 秒**（whisper.cpp tiny，含 ffmpeg 转码） |
| 音频采集 | 系统回环 48 kHz 2ch + 麦克风 48 kHz 1ch |
| release 二进制 | **7.10 MB**（含依赖自检与下载器，见 §9 P0.5） |
| 端到端 | 录 15 秒 → 转写 → 截图关联 → Word(626 KB) → 推送 → 登记 72h 清理，全绿 |

> **启动行为（本轮加固）**：① **单实例保护** —— 重复双击不会叠出一堆进程，
> 第二次启动直接把已开的窗口前置；② **启动即自检依赖** —— 缺 ffmpeg / whisper / 模型
> 会弹窗告知后果并提供一键下载（检查是异步的，不拖慢启动）。

> **前后端已分离（本轮）**：界面全部搬到仓库根的 **`web/`**，改完**刷新即见，
> 不需要重新编译 Rust**（实测：服务运行中改 `web/style.css`，下一次请求就是新的）。
> 接口契约见 **`web/CONTRACT.md`**，负责界面重构的 agent 只读那一份就够。
> 详见 §4.10。

---

## 3. 代码地图

```
D:\VibeClassAgent\
├── web/                             ★ 前端（界面重构改这里，不用碰 Rust）
│   ├── index.html                   入口（后端"前端是否存在"的判据就是这个文件名）
│   ├── style.css                    样式
│   ├── app.js                       逻辑（vanilla JS，无构建步骤）
│   └── CONTRACT.md                  ★ 前后端接口契约（给前端 agent 看的唯一文档）
├── crates/                          Rust 源码（39 个 .rs，约 1.2 万行）
│   ├── vca-core/                    领域核心（无 IO、无 unsafe）
│   │   ├── paths.rs                 ★ 数据/配置目录解析（不落 C 盘那条逻辑在这）
│   │   ├── i18n.rs                  ★ 多语言框架（内置包 + 外部覆盖）
│   │   ├── secrets.rs               本地凭据文件 secrets.env 的读写
│   │   ├── config.rs                配置模型 + save_settings
│   │   ├── job.rs                   作业状态机（Pushed 是清理倒计时唯一起点）
│   │   ├── cleanup.rs               72 小时清理策略
│   │   ├── schedule.rs / time.rs    排课与自研日期算法
│   │   └── import.rs                ClassIsland / CSV 导入
│   ├── vca-ipc/                     插件协议契约（JSON-RPC 消息）
│   ├── vca-platform/                平台适配（唯一允许 unsafe）
│   │   ├── audio.rs                 ★ WASAPI 系统回环 + 麦克风
│   │   ├── capture.rs               ★ 录制会话（ffmpeg + 编码器实测择优 + 收尾合并）
│   │   ├── stt.rs                   ★ 语音转写（本地 whisper.cpp / 云端 API）
│   │   ├── llm.rs                   ★ 要点提取（三模式 + 厂商预设 + list_models）
│   │   ├── browser_bot.rs           CDP 驱动系统 Edge（免费 chatbot 模式）
│   │   ├── docgen.rs                ★ 文档生成（Markdown / Word / PDF）
│   │   ├── shots.rs                 ★ 截图 dHash 去重 + 讲稿时间对齐
│   │   ├── push.rs                  ★ 六种推送渠道 + send_with_retry
│   │   ├── napcat.rs                ★ NapCat（QQ 机器人）识别 / 安装 / 起停 / 日志
│   │   ├── http.rs                  ureq + rustls 封装（代理、连接池、流式下载）
│   │   ├── proc.rs                  静默子进程 + 外部工具查找
│   │   ├── doctor.rs                环境自检
│   │   ├── deps.rs                  ★ 依赖清单与就绪判定（启动自检的判据在这）
│   │   ├── fetch.rs                 ★ 依赖下载补齐（纯 Rust，不依赖 Python 脚本）
│   │   ├── single_instance.rs       命名互斥体，防止重复双击开一堆进程
│   │   ├── window.rs                按标题找窗口并前置（给 GUI 复用）
│   │   ├── tray.rs                  托盘图标（优先从 assets/icon.ico 加载）
│   │   └── overlay.rs               ★ 需求外功能：桌面悬浮窗（见 §9）
│   ├── vca-plugin-host/             插件宿主（清单/发现/进程运行时/本地市场）
│   ├── vca-plugin-example/          示例插件（唯一能跑的插件，照它写插件）
│   ├── vca-engine/
│   │   ├── daemon.rs                守护进程（按课表调度录制）
│   │   └── pipeline.rs              ★ 课后流水线（转写→提取→关联→文档→推送）
│   ├── vca-gui/                     ★ 图形界面（当前主线）
│   │   ├── server.rs                只绑回环 + 一次性令牌 + 托盘 + 开 Edge 应用窗
│   │   ├── api.rs                   /api/* 接口（写操作一律走 patch_settings_line）
│   │   ├── daemon.rs                在界面里起停守护进程
│   │   └── web/                     index.html / style.css / app.js（include_str! 进 exe）
│   └── vca-cli/
│       ├── main.rs                  入口（无子命令时进 repl）
│       ├── repl.rs                  ★ 交互界面（数字菜单 + 斜杠命令 + Claude 配色）
│       └── cmd.rs                   子命令实现（1364 行，文案迁移进行中）
├── locales/                         ★ 语言文件（zh-CN.json / en-US.json）
├── assets/icon.ico                  ★ 托盘图标占位（换 LOGO 直接覆盖此文件）
├── plugins/                         10 个插件目录（1 个已实现，9 个骨架）
├── scripts/
│   ├── env.ps1                      ★ 构建环境准备（dev 与 gate 共用，别再拷第二份）
│   ├── dev.ps1 / dev.cmd            开发入口（编译 + 运行）
│   ├── gate.ps1 / gate.cmd          ★ 提交前三道关（fmt / clippy / test）
│   ├── smoke.py                     ★ GUI 端到端冒烟（起服务打真实 HTTP 接口）
│   ├── fetch-deps.py                下载 ffmpeg / whisper.cpp / 模型到 tools/
│   ├── package.py                   打包发布（自带运行时）
│   └── make-icon.py                 生成占位图标
├── tools/                           ffmpeg + whisper + 模型（约 190 MB，不入库）
└── _tmp/                            一次性脚本（不入库，会被自动清理）
```

---

## 4. 关键决策与理由（**不要重新推翻它们**）

### 4.1 语言选型：Rust，不是 C++

用户问过"比起 Rust 是否推荐 C++"。结论：**不推荐换**，理由：

- 本项目性能瓶颈在 ffmpeg 与 whisper.cpp（都是 C/C++ 写的），**宿主语言的性能差异测不出来**；
- 真正的约束是"无人值守跑一学期"，Rust 的 `Result`/`Send`/`Sync`/所有权直接兑现为可靠性；
- 已经投入 2.3 万行 + 373 个测试 + 三处工具链适配，换语言是负收益。

C++ 只在"进程内自研音视频流水线"上更强，而本项目选择了「ffmpeg 子进程 + 编排层」，
恰好绕开了那个区域。

### 4.2 去 Python 化

上一任用 Python 做算法侧（转写/文档/去重），拖进 269 MB 的 vendor。
**当前架构：纯 Rust**。

- 转写 → whisper.cpp（子进程，官方预编译二进制）
- 文档 → `docx-rs`（Word）+ Edge 无头打印（PDF）
- 去重 → `image` crate 自己算 dHash

> ⚠️ **不要再引入 Python**。用户明确抱怨过旧版"无法启动 Python（os error 267）"。

### 4.3 音频采集走 WASAPI，不走 ffmpeg 的 dshow

旧实现用 `-f dshow -i audio="Microphone"` 有两个硬伤：
中文 Windows 上设备叫「麦克风」匹配不上；`virtual-audio-capturer` 要用户先装虚拟声卡驱动；
而且**采不到系统播放的声音**。

现在用 WASAPI loopback 直接抓渲染端混音，**无需任何驱动**。

### 4.4 录制收尾的中间产物设计

录制期间落盘的是**中间产物**，不是最终文件：

```
<job>/_work_<名>/video.mkv     视频（matroska：被强杀也通常能播；mp4 会丢 moov 而全损）
<job>/_work_<名>/audio/*.wav   系统声 / 麦克风
<job>/_work_<名>/ffmpeg.log    出问题时的现场
```

好处：**收尾失败可重跑，不必重录一节课**。

### 4.5 PDF 走「HTML → Edge 无头打印」

Rust 直写 PDF 遇中文要嵌字体，`printpdf` 之类不做子集化，一份文档动辄带 10 MB 字体。
Edge 用完整 Chromium 排版引擎，中文零配置、体积小。

失败时**保留 HTML** 并在备注里说明，用户可用浏览器另存为 PDF。

### 4.6 浏览器自动化用「差分法」而不是硬编码选择器

站点改版是常态，写死 `#chat-input` 一次改版就报废。现在：

1. 发送前记下 `document.body.innerText` 长度；
2. 发送后轮询，等文本**增长**并连续稳定（判定生成结束）；
3. 取新增段作为回复。

只需要用"可见性 + 位置"启发式找到输入框，不需要知道回复渲染在哪个 DOM 节点。

### 4.8 启动耗时：我们只占 74 ms，剩下全是 Edge

**别再猜了，用数据说话。** 开 `VCA_STARTUP_TRACE=1` 启动即可看到分段耗时。
实测（本机，release）：

```
[STARTUP] serve 进入                     +21.9µs
[STARTUP] HTTP 端口已绑定                 +2.5775ms
[STARTUP] 预览令牌就绪                     +2.9996ms
[STARTUP] 托盘已起                        +73.7543ms
[STARTUP] 界面地址就绪 ← 到达这里用户就能用了  +73.9464ms
```

**我们自己的代码总共 74 ms**，其中 71 ms 是起托盘图标（`CreateWindowEx` +
`Shell_NotifyIcon`）。冷启动到 HTTP 能响应实测 562–1805 ms（受系统负载影响），
多出来的部分主要在进程启动与 DLL 加载。

**用户感觉到的"很久"发生在 Edge 那边**：`--app=` 拉起的 Edge 要
建/校验用户数据目录。那个目录不存在时，Edge 要解压资源、建 SQLite、
跑首次运行检查 —— 这一段是秒级，且**不在我们的日志里**。

所以两条对策：
1. **profile 放稳定位置**（`%LOCALAPPDATA%\VibeClassAgent\ui-profile`），
   这样第二次开始就是热的。**绝不能放 `%TEMP%`** —— 会被清理软件删掉，
   于是每次都要重建一遍，那才是真的每次都慢。
2. **后台预热**（`warm_up_profile`）：在开窗口之前，用一个 `--headless`
   进程先把 profile 目录建出来，**丢到后台线程与其它启动工作并行**。
   注意绝不能串行等它 —— 那只是把慢的动作挪到前面，用户等待的总时长没变。

> ⚠️ **验证边界**：本开发机的沙箱里 Edge **起不来**
> （`FATAL:mojo platform_channel: Check failed (0x5 拒绝访问)`，
> 与 §8 里 PDF 出图失败是同一个沙箱限制）。所以「预热以后到底快了多少」
> **没有实测过** —— 只验证了分段耗时在 74 ms 内、预热不阻塞主流程
> （有测试 `warm_up_never_blocks_or_panics` 守着）、以及进程里确实带了
> 预热逻辑（`--headless` / `about:blank` 在二进制里可查）。
> 真机上请用 `VCA_STARTUP_TRACE=1` 对比一次开窗口的观感。

### 4.9 便携布局 + 引导文件
数据默认放在**程序所在目录**（`data/` 与 `config/`），不碰 C 盘用户目录。
用户可用 `/paths set D:\课堂数据` 换位置，选择记在 exe 同级的 `vca.paths.json`
（不能记在待选择的目录里，那是鸡生蛋问题）。

优先级：`--data-dir` > `VCA_DATA_DIR` > 引导文件 > 便携 > `%LOCALAPPDATA%` 兜底。

### 4.10 前后端分离：`web/` ↔ Rust

**目标**：前端能独立于 Rust 改动。另一个 agent 重构界面时不该碰 `crates/`，
更不该每改一行 CSS 就等一次 `cargo build`。

**做法**是「运行时资源查找 + 内置兜底」两层，**不是**把前端拆成独立进程/仓库：

| 层 | 位置 | 作用 |
|---|---|---|
| 外部 | 仓库根 / exe 同级 / `VCA_WEB_DIR` 的 **`web/`** | 开发与界面重构走这条，**改完刷新即见** |
| 内置 | `crates/vca-gui/src/web/`，`include_str!` 编进 exe | 兜底，保证 exe 单独拷走也能开界面 |

查找顺序见 `crates/vca-cli/src/main.rs` 的 `resolve_web_dir()`。

**为什么不做成独立前端工程**：这是要部署到一体机上的桌面程序，
目标机器上**不装任何运行时**是硬约束（HANDOFF 开头就写了"纯 Rust"）。
引入 node 构建链会把这条底线破掉。所以分离的边界划在**文件系统**，
而不是"两个进程"。想加构建步骤也行，但产物必须落在 `web/` 下且浏览器能直接跑。

**服务端为此改了什么**（原来只认死三个文件名，前端多拆一个文件就得回来改 Rust）：

- 外部目录下的**任意**文件都能被服务，支持子目录（`components/table.js`）；
- 文件名 → `Content-Type` 走一张小表（`web.rs::content_type`），加新类型不用改 match；
- **路径穿越必须挡住**：外部目录是可控的，`/../config/...` 能把带
  `preview_token` 的配置读出来。`safe_join()` 拒绝 `..`、绝对路径、盘符、UNC ——
  有 6 个测试专门守这条（含 `%2f` 编码绕过）。

**验证过的**（本轮实测）：

```
外部新文件 /_probe.txt          → 200 'EXTERNAL-OK'  text/plain
子目录 /sub/mod.js              → 200               application/javascript
/../config/settings.example.yaml → 404（挡住）
/..%2fconfig%2f...yaml          → 404（挡住）
/sub/../../Cargo.toml           → 404（挡住）
Cache-Control                   → no-store
服务运行中改 web/style.css       → 下一次请求就是新内容（exe 时间戳未变）
```

**后端要同步的事**：前端改了 `web/` 之后，后端把它同步一份到
`crates/vca-gui/src/web/`（内置兜底）再重编译。这步由后端负责，
前端 agent 不用管 —— 但**别忘了**，否则"exe 单独拷走"会退化成旧界面。

> 契约文档在 **`web/CONTRACT.md`**（40 个接口、返回结构、i18n 约定、
> `need_notice` 这类"后端已经替你算好"的字段）。给前端 agent 看那一份就够。

---

## 5. 踩过的坑（**务必不要重复**）

### 5.1 环境类（本机特有，已写进 `scripts/dev.ps1`）

| # | 坑 | 症状 | 解法 |
|---|---|---|---|
| 1 | **raw-dylib 链接失败** | `dlltool.exe: CreateProcess` | rustup 的 GNU 工具链**自带 dlltool 却不带 as.exe**，而 dlltool 忽略 `AS` 环境变量、只在自己目录找 `as`。把 as.exe 复制到 `self-contained/` 目录即可 |
| 2 | **ring 编译失败** | `failed to find tool "gcc.exe"` | 需要 C 编译器，但**绝不能把 MinGW 放进 PATH**（它的 ld 会被 rustc 当链接器，报 `cannot find crt2.o / -lwinhttp`）。只用 `CC`/`AR` 环境变量告诉 cc-rs |
| 3 | **.ps1 中文注释导致语法错** | `Unexpected token` | Windows PowerShell 5.1 按 ANSI 读无 BOM 的 .ps1，中文变乱码破坏语法。**编辑任何 .ps1 后必须补 UTF-8 BOM** |
| 4 | **cargo 连不上 crates.io** | schannel `SEC_E_NO_CREDENTIALS` | 本机 schannel 被阻断。用 `scripts/regproxy.py` 本地 HTTP 代理转发（需系统代理 10818 在跑） |
| 5 | **PowerShell 内存爆** | `Allocation failed` | 8 GB 机器上 `cargo test` 会同时编译 lib + lib-test。用 `-j 1`，或设 `CARGO_PROFILE_DEV_DEBUG=0` |
| 5b | **`$ErrorActionPreference="Stop"` 会吃掉原生程序的 stderr**（本轮新增） | 脚本在半路被异常打断：该跑的检查没跑完，结尾也不打印结论，只留一屏编译输出 | cargo 的编译错误与 clippy 告警**全都走 stderr**，PS 会把它当成 terminating error。调外部命令前把偏好切回 `Continue`，用 `$LASTEXITCODE` 判成败（见 `scripts/gate.ps1` 的 `Invoke-Cargo`）。实测第一次跑 gate 就栽在这：报「未通过：fmt / test」，而真正的 clippy 压根没跑完 |
| 5c | **新增依赖时 cargo 会去连本地 sparse 代理**（本轮新增） | 反复刷 `spurious network error ... 127.0.0.1:13579`，看起来像卡死（实测卡了一次 gate） | 代理没在跑。收窄依赖特性 + `--offline` 是更常用的一条路：`zip` 的 default 会拖进 aes / bzip2 / lzma / zstd / xz 一堆本机缓存里没有的 crate，关掉 default 只留 `deflate-flate2` 就过了 |

### 5.2 代码类（血泪教训）

| # | 坑 | 教训 |
|---|---|---|
| 6 | **`ffmpeg -encoders` 里有名字 ≠ 能用** | i5-10400 上 `h264_qsv` 在列表里，但 `Error creating a MFX session: -9`，结果录出 **0 字节**的 mkv，直到收尾才报 EBML 解析失败 —— **一节课白录**。必须**真编一帧**探测（`probe_encoder`） |
| 7 | **ffmpeg 的 stderr 不能丢** | 曾经设为 null，那个 0 字节文件查了很久。现在落盘到 `_work/ffmpeg.log`，0 字节时直接把日志尾部打进结论 |
| 8 | **无音轨时不能给 `-c:a`** | ffmpeg 报「参数未用于任何输出流」，连纯视频录制也收尾失败 |
| 9 | **rustyline 的 prompt 不能带 ANSI** | Windows 上直接断言失败 `content should not be styled directly` —— **不是显示异常，是进程崩溃**。提示符必须是纯文本 |
| 10 | **截图的闭区间陷阱** | 时间轴相邻两段常首尾相接（上一段 end == 下一段 start），闭区间会让边界点被前一段抢走 —— 截在第 60 秒的图配上第 0~60 秒的讲稿。用半开区间 `[start, end)` |
| 11 | **`.gitignore` 的 `*secrets*` 误伤源码** | 把 `secrets.rs` 一起排除了，导致该文件静默不入库。必须补 `!secrets.rs` 例外，且例外要写在规则**下面** |
| 12 | **cargo init 会污染 workspace** | 在 `_tmp/` 里 `cargo init` 会把该目录加进根 `Cargo.toml` 的 members。已加 `exclude = ["_tmp"]`，但 cargo 仍会改写文件——**不要在 workspace 内做探针项目** |
| 13 | **短名 `t()` 撞局部变量** | 批量迁移文案时生成的 `t("key")` 与源码里的 `let t = ...` / `Ok(t) => ...` 撞名，报 `expected function, found &str`。**批量改动必须用全路径 `vca_core::i18n::t(...)`** |
| 14 | **模板占位符被当成已配置** | 配置模板里写的是 `<模型名>`，既非空也不是合法值，界面显示成"已配置"。需要 `is_placeholder()` 识别 |
| 15 | **`cargo run` 时数据落到 target/debug** | `cargo clean` 一跑全没。已识别「exe 在 target/{debug,release}」并上溯到仓库根 |
| 19 | **正在录制的作业被当成待办捞走**（本轮新增） | 录制一开始就把作业标成 `Recorded` 存盘，它立刻出现在 `pending()` 里。若处理窗口与上课时段重叠，**没写完的录像会被捞去处理**，报「转写失败：没有可转写的音视频文件」并标 Failed。已让 `process_scope` 排除正在录制的作业 |
| 20 | **改了课表时段，已录好的作业就再也匹配不上处理窗口**（本轮新增） | `job_id` 里嵌着**录制开始时刻**（`...T0113...`），而 `process_scope` 是用**当前课表**反算 `job_id` 的。时段一改（临时调课、改作息表、事后补课表），`targets` 非空却不含那个作业 → todo 为空 → **静默返回，无日志无报错**，看上去就像「处理窗口根本没用」。`targets.is_empty()` 的兜底只在当天完全没课表条目时才生效。**排查手段**：用 `window_at` 里的窗口名 + 作业 `job.json` 的 `start` 字段对一遍。改进方向见 §9.1 |

| 21 | **`items_after_test_module` 只有 clippy 拦得住**（本轮新增） | `cargo check --all-targets` 全绿、`cargo test` 也全绿，唯独 `cargo clippy -- -D warnings` 报错：测试模块后面不能再有别的定义。**这就是「三关必须都跑」的实证** —— 少了 clippy 这一关，这个错会一路带进提交。已固化成 `scripts/gate.cmd` |
| 24 | **本地 sparse 代理不转发 `/dl/`，cargo 会报 404 NoSuchKey**（本轮新增） | crate 元数据在 index.crates.io，**包体在 static.crates.io**。代理只转发 index 时，cargo 请求 `<代理>/dl/{name}/{version}/download` 会拿到 `NoSuchKey`，而报错里只出现代理地址，很容易误判成「代理没起来」。要把 `/dl/{crate}/{version}/download` 映射成 `static.crates.io/crates/{crate}/{crate}-{version}.crate`（见 `scripts/regproxy.py`） |
| 25 | **`cargo test -- --ignored` 这类参数会被 PowerShell 吃掉**（本轮新增） | `-File` 模式下 `--` 之后的参数绑定会失败（`A positional parameter cannot be found`）。给包装脚本加一个专门的 `.ps1` 再 `-File` 跑，别在命令行上拼 `--` |
| 22 | **前端资源是 `include_str!` 打进 exe 的**（本轮新增） | 改完 `app.js` 打开界面没变化，人会先怀疑缓存、再怀疑后端。真相是 exe 里那份还是旧的：**改前端必须重新编译**。开发时想免编译，把文件放到 `<程序目录>/assets/web/` 下（`web::resolve` 优先外部） |
| 23 | **登录二维码被丢进 `nul` 就永远扫不到**（本轮新增） | 按「静默启动」的惯例把 NapCat 子进程的 stdout 设成 null，用户看到的是「启动了但一直连不上」的黑箱。**该静默的是窗口，不是信息**：现在输出落到 `logs/vca-launcher.log`，界面直接把日志尾巴显示出来 |
| 26 | **「文件大小的下限」会把健康的机器判成缺依赖**（本轮新增） | 判「依赖在不在」时我顺手加了个体积下限来抓「下了一半的残缺文件」，一度把 `whisper-cli.exe` 的下限定成 512 KB —— 但它真实只有 **469 KB**，于是**健康机器上误报缺依赖**，理由还是「文件不完整（0 MB，至少应有 0 MB…）」这种废话。**下限的唯一职责是识别碎片，不是猜正常体积**；定得太高就是在给正常环境泼脏水。真实体积已钉进测试，并加了 `humanize()` 让小于 1 MB 时按 KB 报 |
| 27 | **`--disable-features` 在一串参数里只能出现一次**（本轮新增） | 给 Edge 传启动参数时写了两处 `--disable-features=`，**后者不会合并、而是直接覆盖前者**，导致前一批标志静默失效。要合并成一条逗号分隔的列表。已抽成 `edge_args()` 并用测试钉住 |
| 28 | **验证脚本自己写错路径，比产品出错更费时间**（本轮新增） | 查「只提示一次」的标记文件时，我的探针脚本去 `data/config/` 下找，实际写在 `config_root`（`config/.deps-notified`）。看到「标记没写」差点去改代码 —— 那是**测试错了，代码是对的**。**结论：报错先怀疑探针**，用真实的挪文件 / 读接口方式复核一遍再动产品代码 |
| 29 | **磁盘满会伪装成「代码有 bug」**（本轮新增） | 9 个工作区测试突然失败（`shots::*` 7 个 + `docgen` + `fetch`），报错五花八门（`ENOSPC`、`memory allocation failed`、`StorageFull 磁盘空间不足`），看着像逻辑错误。真相是 **C 盘只剩 0.07 GB**（`%TEMP%` 就在 C 盘）。清掉 `target/debug/incremental` 并把 `TEMP`/`TMP` 指到 D 盘后，**9 个失败全部消失、373 全绿**。**排查建议**：一次冒出一堆互不相干的失败时，先量磁盘余量再读代码 |
| 30 | **`git checkout -- <file>` 会连未提交的改动一起冲掉**（本轮新增） | 为了插一行计时探针，我改完发现中文注释被 PowerShell 按 GBK 读坏了，于是 `git checkout -- server.rs` 想"还原到干净状态" —— 结果 **Task F 那 380 行未提交的改动全没了**（775 行 → 396 行）。**教训**：这个仓库里大量工作是"未提交"状态，`git checkout` / `git restore` / `git stash` 都不是"撤销我刚才那一下"，而是"回到上次提交"。正确做法是重做那一次编辑（或用更窄的手段），**不是 checkout**。恢复路径：`target/release/vca.exe` 里有完整代码，`.o` 文件里有全部字符串常量、函数符号与测试名 —— 靠这两样能精确重建 |
| 31 | **PowerShell 改 `.rs` 文件会把中文注释毁掉**（本轮新增） | `Get-Content -Raw` 默认按 ANSI/GBK 解码 UTF-8，`Set-Content` 再写回去，中文全部变成 `鍚姩鏈嶅姟` 这样的乱码，而且 `cargo check` 之前看不出来。**改源码一律用编辑工具或 Python**（`encoding='utf-8'`），别用 PowerShell 的读写命令做"文本替换" |
| 32 | **`.gitignore` 漏了运行期配置，真实课表差点进仓库**（本轮新增） | 提交前 `git status` 里冒出 `config/profiles/`、`config/schedule/`、`config/timetable/`，本来想"顺手 add 进去"。查了内容才发现：**`settings.yaml` 里有长期有效的 `preview_token`，`schedule/current.yaml` 里有真实教师姓名**。差点推上 GitHub。**教训**：① `config/` 下"程序真正读写的那一份"必须靠 `.gitignore` 挡住（只放行 `*.example.yaml`），不能指望自己记得住；② 提交前 `git diff --cached` 搜一遍 `token`/人名。**已修**：见 `.gitignore` 的「用户的真实配置与课表」一节 |

### 5.3 PowerShell / git 用法类

| # | 坑 | 解法 |
|---|---|---|
| 16 | 当前目录的脚本要 `.\` 前缀 | 否则报 `The module 'scripts' could not be loaded`（它把 `scripts` 当模块名找） |
| 17 | commit message 里的中文标点会被 PowerShell 解析坏 | 报 `pathspec '可换' did not match any file(s)`。**把 message 写到文件，用 `git commit -F`** |
| 18 | 推 GitHub 时 TLS 偶发 `unexpected eof` | 代理不稳。**带退避重试**（通常第 2-3 次成功）。注意：报错≠没推上去，重试时可能显示 "Everything up-to-date" |

---

## 6. 本机环境（当前这台机器）

```
Windows 11 ｜ 项目在 D:\VibeClassAgent ｜ Git 在 D:\Git
无 MSVC（只有 Dev-Cpp 的 MinGW64，GCC 4.9）
Rust 工具链装在项目内 .toolchain/（GNU 目标，因为无 link.exe）
系统代理 127.0.0.1:10818（v2rayN，重启电脑后需等它起来）
```

**构建**（`scripts/dev.ps1` 已封装全部适配，直接用即可）：

```powershell
cd D:\VibeClassAgent
.\scripts\dev.cmd              # 编译并进交互界面
.\scripts\dev.cmd -Check       # 只做 cargo check
.\scripts\dev.cmd doctor       # 环境自检
.\scripts\dev.cmd debug e2e    # 端到端冒烟
```

> PowerShell 里**当前目录的脚本必须加 `.\`**；不想 cd 就用
> `& 'D:\VibeClassAgent\scripts\dev.cmd' ...`。

**直接跑 cargo**（需要手动设环境变量）：

```powershell
$env:CARGO_HOME='D:\VibeClassAgent\.toolchain\cargo'
$env:RUSTUP_HOME='D:\VibeClassAgent\.toolchain\rustup'
$env:CC='D:\Dev-Cpp\MinGW64\bin\gcc.exe'
$env:AR='D:\Dev-Cpp\MinGW64\bin\ar.exe'
$env:PATH="$env:CARGO_HOME\bin;$env:PATH"
# 剔除 Dev-Cpp，否则它的 ld 会被 rustc 当链接器
$env:PATH = (($env:PATH -split ';') | Where-Object { $_ -and $_ -notlike '*\Dev-Cpp\*' }) -join ';'
cargo test --workspace -j 1     # 8 GB 机器建议 -j 1
```

---

## 7. 工作流程约定

### 7.1 提交前必过三关

```powershell
.\scripts\gate.cmd        # 一次跑完下面三条，全过才提交
```

等价于：

```powershell
cargo fmt --all -- --check               # 不合规就跑 cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings   # 别去掉 -D warnings
cargo test --workspace
```

> 三条手工敲很容易漏掉 clippy，而 clippy 恰恰是唯一能拦住
> `items_after_test_module`（见坑 #21）这类问题的关口。脚本存在的意义就是这个。
> `gate.cmd` 与 `dev.cmd` 共用 `scripts\env.ps1` 的环境准备，
> 改工具链设置只改那一处，免得两边漂移成「开发能跑、gate 说找不到 gcc」。

### 7.2 提交信息

- **写清「为什么这么改」，不只是「改了什么」**；踩到的坑要写进正文。
- 用文件传 message（见坑 #17）：

```powershell
# 先写 _tmp/commit-msg.txt，然后
git add -A; git commit -q -F _tmp/commit-msg.txt
```

### 7.3 推送到 GitHub

仓库：`https://github.com/quxianp/VibeClassAgent.git`（分支 `main`）

**凭据在 Windows 凭据管理器里**（GCM 被沙箱的命名管道限制挡死，只能直接读）：

```powershell
$src = @'
using System;
using System.Runtime.InteropServices;
public class CredX {
  [DllImport("advapi32.dll", SetLastError=true, CharSet=CharSet.Unicode)]
  public static extern bool CredRead(string target, int type, int flags, out IntPtr credential);
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
  public struct CRED {
    public int Flags; public int Type; public string TargetName; public string Comment;
    public long LastWritten; public int CredentialBlobSize; public IntPtr CredentialBlob;
    public int Persist; public int AttributeCount; public IntPtr Attributes;
    public string TargetAlias; public string UserName;
  }
  public static string[] Get(string target) {
    IntPtr p;
    if (!CredRead(target, 1, 0, out p)) return new string[]{"ERR",""};
    var c = (CRED)Marshal.PtrToStructure(p, typeof(CRED));
    return new string[]{ c.UserName, Marshal.PtrToStringUni(c.CredentialBlob, c.CredentialBlobSize/2) };
  }
}
'@
Add-Type -TypeDefinition $src
$r = [CredX]::Get("git:https://github.com")
$tok = $r[1]
$url = "https://" + $r[0] + ":" + $tok + "@github.com/quxianp/VibeClassAgent.git"
for ($i=1; $i -le 5; $i++) {
  $o = git -c credential.helper= push $url main 2>&1
  $c = $LASTEXITCODE
  ($o | ForEach-Object { $_ -replace [regex]::Escape($tok), '***' }) | Select-Object -First 2
  if ($c -eq 0) { "PUSH OK"; break }
  Start-Sleep -Seconds 8
}
```

> 注意：`git config` 里已设 `http.proxy=http://127.0.0.1:10818` 与 `http.sslBackend=openssl`
> （schannel 在本机不可用）。**推送前确认代理在跑**（`Test-NetConnection 127.0.0.1 -Port 10818`）。
> 输出里务必把 token 替换成 `***`，别把凭据打进日志。

### 7.4 临时文件

一次性脚本、探针、抓取结果一律放 `_tmp/`（已 gitignore，会被自动清理器清掉）。
**正式代码和文档不要放进 `_tmp/`。**

### 7.5 不要动的东西

- `dist/`：发布包产物，用 `scripts/package.py` 生成，不要手工编辑
- `.toolchain/`：本机 Rust 工具链（958 MB，不入库）
- `python/`：**已被去 Python 化废弃**（vendor 269 MB 仍在磁盘上，不参与构建）。
  如果确认没用了可以删，但**先确认 `dist/` 与 `scripts/` 没有引用它**

---

## 8. 已验证 vs 未验证（诚实清单）

### ✅ 真机实测通过

- 录制：屏幕 + 系统声音 + 麦克风，含硬件编码器实测择优与失败降级
- 转写：whisper.cpp tiny，10 秒音频 1.0–1.4 秒
- 截图关联：dHash 去重 + 与讲稿时间对齐
- 文档：Markdown 总生成 + Word（含插图，实测 626 KB）
- 推送：dry-run 路径
- 清理：登记 72 小时计划
- 端到端：`vca debug e2e` 跑通，结束状态 `Pushed`
- 插件：`vca plugin list` 列出 10 个；示例插件五个方法全部应答正确
- **质量关（本轮）**：`scripts\gate.cmd` 三关全过 —— fmt 合规、clippy（`--workspace --all-targets -- -D warnings`）零告警、**373 个单元测试全绿**（2 个 ignored 是需要外网的）
- **启动依赖自检（本轮新增）**：用真实挪走依赖文件的方式端到端验过三条路径 ——
  ① 全就绪时 `missing=0 need_notice=0`（不会打扰用户）；
  ② 缺 `ffmpeg` + 模型时 `missing=2 notice_required=true`，逐项给出「文件不存在」；
  ③ **严重程度分级确实生效**：必需项 `ffmpeg` 每次启动都提示，可选项模型在用户点过「稍后」后写标记文件、后续启动静默
- **单实例保护（本轮新增）**：命名互斥体（`Local\...`）防止重复双击开出一堆进程；进程任何死法都由内核释放，不会留下"假锁"
- **GUI 形态（本轮）**：`python scripts\smoke.py` **58 项全过** ——
  静态资源、令牌保护（错误 token 403）、配置写入与回读、真实发送测试消息
  （自带 mock OneBot 收报文并校验内容）、时间表推导处理窗口、
  **课表时间由时间表补全**、**上课时间点清单**、**绑定与触发规则落盘**、
  ClassIsland 课表导入、**导入新增时间表而非覆盖**、逐条「录制」开关落盘、
  界面文案接口、机器人探测、守护进程起停、作业与清理预览、退出接口
- **daemon 全链路（2026-09-20 真实时间轴实测）**：到点自动开录 → 录制中拒绝处理
  → 收尾成 mp4 → 处理窗口内自动跑完转写 / 截图关联 / 生成文档 / 推送 / 登记 72 小时清理。
  终点作业状态 `PUSHED`、`push_succeeded_at` 有值、`expire_at` = +72h、`last_error` 为空；
  产物 Word 25 KB + HTML + Markdown（PDF 因沙箱限制落空，见下表）

### ❌ 未验证（**接手时不要假装它们好了**）

| 项 | 为什么 | 怎么验证 |
|---|---|---|
| **PDF 生成** | 在我这边**必然失败**且不是代码问题：沙箱禁命名管道，Chromium 报 `FATAL:mojo platform_channel: Access is denied`，退出码 `0x80000003`。已有单进程重试 + HTML 兜底（实测同一次运行里 Word/HTML/Markdown 都正常，只有 PDF 落空）。**普通终端里能不能出图没验过** | 在普通 PowerShell 里跑 `/process`，看 `<日期>课堂纪要.pdf` 是否生成 |
| **真实模型提取** | 验证时没配 `VCA_LLM_API_KEY`，走了降级分支（`要点提取降级为原文摘要`）—— 降级路径本身验过了，模型路径只在 `debug e2e` 里用小样本验过 | 配好 Key 后跑一次完整课后处理 |
| **浏览器自动化模式** | 没有真实站点账号可测 | 需用户提供，或用 `selectors` 覆盖调试 |
| **QQ / 微信推送真实通道** | 没有真实的机器人凭据 | 需用户配置后试 |
| **一体机上课时段长跑** | 无真实环境 | 建议先跑一周观察磁盘与日志 |
| **托盘图标实际显示** | 代码与加载都验过（有测试），但"在任务栏里看得见"需要人眼确认 | 跑 `vca gui`，看右下角与任务栏 |
| **企业微信智能机器人（aibot）的完整链路**（本轮新增） | 没有真实的企业微信机器人凭据。**但协议实现已经真连验证过**：用假凭据连 `wss://openws.work.weixin.qq.com`，拿到了 `errcode=853000 invalid bot_id or secret` —— 说明 TLS 握手、WebSocket 协议升级、认证帧构造、`req_id` 回执匹配全部正确，只差真凭据。发送帧（`aibot_send_msg`）之后的部分没验过 | 在企业微信后台建一个智能机器人，填上 ID / Secret / 会话 id 后点「发送测试消息」 |
| **NapCat 的真实下载**（本轮新增） | 开发机**连不上 GitHub**（直连超时、本地代理没在跑），三个下载源一个都验不到。识别 / 解压 / 起停 / 日志这些**逻辑**用构造的假安装目录测过（含 17 个单元测试），但「真的能从 GitHub 下到 60 MB 的包」没验过 | 在能上网的机器上点一次「一键安装」，确认 `tools/napcat/` 里出现 `launcher.bat` 一类入口 |
| **NapCat 真实启动与扫码登录**（本轮新增） | 没有真实 NapCat 包，也没有可登录的 QQ 小号 | 装好之后点「启动」，看日志里是否出现二维码、扫码后 OneBot 端口（默认 3000）是否开始监听 |
| **依赖补齐的真实下载**（本轮新增） | 与 NapCat 下载同一个原因：开发机连不上外网，`fetch.rs` 里 7 个下载源（gyan.dev / BtbN / GitHub / ghfast.top / hf-mirror / huggingface）一个都没实连过。**已验证的是**：下载器的 URL 表、镜像降级顺序、解压后按文件名在树里找目标、DLL 同伴搬运、失败隔离、临时目录落盘再原子搬运、通知策略的严重程度分级 —— 这些都有单元测试（22 个），并且「缺文件时 API 返回什么」用真实挪走文件的方式端到端验过。**没验过的是「真的能把 125 MB 下下来并装到位」** | 在能上网的机器上，把 `tools/ffmpeg/ffmpeg.exe` 改名，启动 GUI，点「确定」，看进度条走完后文件是否回位 |

---

### 📐 课表模型与 ClassIsland 的逐项对照（本轮重构）

时间表与课表的**数据结构、编辑交互、调度模型**三条都对齐了 ClassIsland。
对齐依据是它的官方文档与 API 文档（不是猜的）：

- [时间表 | ClassIsland 文档](https://docs.classisland.tech/app/profile/time-layout.html)
- [课表 | ClassIsland 文档](https://docs.classisland.tech/app/profile/classplan.html)
- [ClassIsland.Shared.Models.Profile 命名空间 | API 文档](https://api.docs.classisland.tech/api/ClassIsland.Shared.Models.Profile.html)

#### 一、数据结构逐项对照

| ClassIsland | VCA 对应 | 位置 | 说明 |
|---|---|---|---|
| `Profile.TimeLayouts.<guid>` | `Timetable` | `model.rs` | 可有多份；`is_active` 标当前启用 |
| `Profile.ClassPlans.<guid>` | `ClassPlan` | `model.rs` | 可有多份 |
| `TimeLayout.Layouts[]` | `Timetable.slots[]` | `model.rs` | 一天的时间点序列 |
| `TimeLayoutItem.StartTime` / `EndTime` | `TimetableSlot.start` / `end` | `model.rs` | `HH:mm` 字符串 |
| `TimeLayoutItem.Last`（算出来的时长） | `TimetableSlot::duration_minutes()` | `model.rs` | 由结束减开始算出，不落盘 |
| `TimeLayoutItem.TimeType`（0 课 / 1 课间 / 2 分割线 / 3 行动） | `SlotKind::{Class, Break}` | `model.rs` | **只实现 0 与 1**，2/3 见下「刻意的差异」 |
| `TimeLayoutItem.BreakName` | `TimetableSlot.name` | `model.rs` | 课间名称 |
| `TimeLayoutItem.DefaultClassId` | `TimetableSlot.default_subject` | `model.rs` | 该时间点的默认科目 |
| `TimeLayoutItem.IsHideDefault` | `TimetableSlot.is_hidden` | `model.rs` | 默认隐藏 |
| `ClassPlan.TimeLayoutId` | `ClassPlan.time_layout_id` | `model.rs` | 课表绑定哪份时间表 |
| `ClassPlan.TimeRule.WeekDay`（1=周一…7=周日） | `TimeRule.weekday` | `model.rs` | `0` 表示每天 |
| `ClassPlan.TimeRule.WeekCountDiv` / `WeekCountDivTotal` | `TimeRule.week_count: CycleRule{week,total}` | `model.rs` | 多周轮换 |
| `ClassPlan.IsEnabled` | `ClassPlan.is_enabled` | `model.rs` | 是否默认启用 |
| `ClassPlan.Classes[]` | `ClassPlan.classes[]`（`DayClasses.slots[].ClassSlot`） | `model.rs` | 按星期分块、按时间点索引 |
| `ClassInfo.Index`（课程在课表中的位置） | `ClassSlot.period` | `model.rs` | 落在第几个**上课**时间点 |
| `ClassInfo.SubjectId` | `ClassSlot.subject` | `model.rs` | VCA 直接存科目名，省掉一层 GUID 间接 |
| `ClassInfo.IsEnabled` | `ClassSlot.record` | `model.rs` | VCA 这里还兼作「这节课录不录」 |
| `ClassPlan.ValidTimeLayoutItems`（只保留上课类型的时间点） | `Timetable::class_slots()` | `model.rs` | 课程格的列头就是它 |
| `ClassPlanGroup` | `Timetable.group` | `model.rs` | 仅用于归档展示 |
| 「时间表必须没有被任何课表使用才能删」 | 删除保护 | `app.js` + `post_timetable` | 同样拒绝删除在用的时间表 |

#### 二、交互逐项对照

| ClassIsland 的做法 | VCA 实现 | 位置 |
|---|---|---|
| 编辑窗口左侧选时间表、中间编辑、右侧看详情 | `.tt-wrap` 三栏主从布局 | `app.js` `renderTimetable()` |
| 时间表编辑器有**列表视图**与**时间轴视图** | 表格 + 按分钟数占宽的 `.tl-bar` 时间轴预览 | `app.js` / `style.css` |
| 新增上课点默认 40 分钟、课间默认 10 分钟 | `addRow` / `data-add` 同默认值 | `app.js` |
| 新时间点默认接在已有时间点之后 | 插入时 `start = 上一段.end` | `app.js` |
| 选中时间点后拖动开始/结束把柄改时间 | 两条路都给了：表格里改「开始 / 结束 / 时长」三者自动联动；时间轴上**直接拖** —— 拖两端改起止、拖中间整段平移（按段内相对位置判断，因为窄段只有几分钟宽，靠把手抓不准；5 分钟一档） | `app.js` / `style.css` |
| 「时间点信息」里改名称与默认科目 | 右侧详情栏：默认科目 + 默认隐藏 | `app.js` |
| 时间点类型可选（上课 / 课间 / 分割线 / 行动） | 上课 / 课间两档（见「刻意的差异」） | `app.js` |
| 课表按时间点对齐填课程（列=时间点、行=星期） | `.sc-grid` 课程格，列头带节次与时长 | `app.js` `renderSchedule()` |
| 科目来自统一的科目库 | `<datalist>` 候选来自课表里已出现的科目名 | `app.js` |
| 课表触发规则（星期 + 单双周）在编辑窗口里设 | 课表页顶部的「时间表 / 触发：星期 / 周次 / 默认启用」四个控件 | `app.js` |
| 删除正在被课表使用的时间表会被拒绝 | 前端查 `/api/schedule` 后拒绝；后端也校验绑定是否存在 | `app.js` / `api.rs` |

#### 三、调度模型

- ClassIsland：`TimeRule` 全满足 → 该课表激活 → 用它的 `TimeLayoutId` 找到时间表 →
  `Classes[i]` 与第 i 个**上课**时间点一一对应。
- VCA：`ScheduleFile::normalize()` 把 `classes`（或旧格式 `entries`）统一摊平成
  `entries`，`start`/`end` 由绑定的时间表补出；随后
  `schedule::plan_for_date()` 照旧按下标/时间算课。
  **对外行为完全没变** —— 只是课表不再要求用户手抄起止时间。
- 落点：`crates/vca-core/src/config.rs::ScheduleFile::normalize`，
  以及 `fill_from_timetable`（节次 → 时间、时间 → 节次双向补全）。
- **触发规则真的参与排课**（不只是落盘校验）：
  - `ScheduleFile::plan_for_weekday` 先用 `TimeRule.WeekDay` 判断这份课表当天是否生效
    （`0` = 每天，是默认值）—— `WeekDay` 在 ClassIsland 里是**整份课表**的触发条件，
    不是某一条课的属性，所以判断放在这一层
  - `schedule::plan_for_date` 再用 `TimeRule` 判一遍「星期 + 生效日期区间 + 周次轮换」
  - 周次来源是 `WeekParity`：`week_no(rule)` 只在课表确实是「2 周一轮」时
    给出周次（单周=1、双周=2）；其余情况返回 `0` 表示「不知道第几周」，
    `CycleRule::matches_week` 会放行 —— **宁可照常上课，也不要因为算不出周次而静默漏课**
    （三周及以上的轮换要等「当前是第几个教学周」有来源时再补）

#### 四、旧数据自动升级（不丢东西）

- 读到旧格式（`entries` 只有起止时间、没有节次）时自动补出节次，
  **原文件另存为 `current.yaml.bak`**，然后就地升级。
- 判据保守：只有确实能补出信息才动文件；格式已经对的配置一个字节都不改。
- 代码：`crates/vca-core/src/import.rs::upgrade_schedule_file`
  （`needs_upgrade` 先试跑一遍再决定要不要落盘）。
- daemon 与 GUI 两条读盘路径都会触发，用户不需要手工改 YAML。

#### 五、刻意的差异（**不是漏做**）

| 项 | 为什么不做 |
|---|---|
| `TimeType` 2（分割线）/ 3（行动） | VCA 没有「自动化行动」这一层，分割线只是显示效果；实现它只会多两个永远不触发的分支 |
| `ClassPlan.IsOverlay` / `OverlaySourceId`（临时层） | VCA 用 `Overrides`（`cancel` / `cancel-one` / `add` / `move`）表达当天的临时调整，语义等价且更简单 |
| `ClassPlanGroup` 的群优先级加载 | VCA 单教师、单套课表，`group` 只留作归档展示 |
| `TimeRule` 缺省时「所有课表都不激活」 | VCA 是 `week_template` 直接生效（没有多套并存），否则用户排完课却发现「今天没课」，比不激活更难解释 |
| 科目单独一张 GUID 表 | VCA 直接用科目名，避免在单机场景里多一层查表 |

---

## 9. 未完成事项与优先级

按价值排序（**建议按这个顺序做**）：

### ✅ P0.5：启动依赖自检 + 一键补齐 —— 已完成（本轮）

**要解决的问题**：程序拷到一体机上，用户不会装 ffmpeg、不会下 whisper 模型。
缺了它们，录制能跑但转写/文档全废，而**报错信息藏在日志里，老师根本看不到**。

**做法**（三块，都是纯 Rust，不依赖 `scripts/fetch-deps.py`）：

1. `crates/vca-platform/src/deps.rs` —— 依赖清单与判据。
   三个条目：`ffmpeg`（**必需**）、`whisper`、`model`（后两个**可选**，缺了只降级本地转写，
   云端兜底仍在）。判定不只看"文件在不在"，还看**大小是否低于下限** ——
   这是为了抓住「下了一半的残缺文件」。
   > ⚠️ **下限必须远低于真实体积**。这里踩过一个坑：`whisper-cli.exe` 真实只有
   > **469 KB**，而下限一度写成 512 KB，导致**健康机器上误报缺依赖**。
   > 下限的唯一职责是"识别碎片"，不是"猜正常体积"。真实体积已钉进测试
   > （`min_bytes_leaves_room_for_real_files`）。附带加了 `humanize()`，
   > 小于 1 MB 时用 KB 显示，避免出现「至少应有 0 MB」这种鬼话。
2. `crates/vca-platform/src/fetch.rs` —— 下载器。
   每个组件配**多个镜像**按序降级（ffmpeg：gyan.dev → BtbN GitHub；
   模型：hf-mirror → huggingface；whisper：GitHub → ghfast.top）。
   先下到 `_fetch_tmp/` 再搬到位，**中途失败绝不留下半个文件**；
   单个组件失败不影响其它组件；末尾**重新检查一遍**并返回真实状态（不是假成功）。
   上游 zip 有时多套一层目录，所以用 `find_in_tree()` 按文件名递归找，而不是写死路径。
3. GUI 侧 —— `GET /api/deps` 报告状态；`POST /api/deps/fetch` 触发下载并轮询进度；
   `POST /api/deps/dismiss` 记账。`boot()` 里**不 await**，所以检查过程不阻塞界面。

**提示策略（按严重程度分级 —— 这是用户明确要的）**：

| 缺失项 | 提示时机 |
|---|---|
| `ffmpeg`（必需） | **每次启动都提示** —— 缺了它程序基本残废，不能让它被静默掉 |
| `whisper` / `model`（可选） | **只提示一次**，用户点过「稍后」后写 `config/.deps-notified`，后续不再打扰 |

> 注意标记文件在 **`config_root`**（即 `config/.deps-notified`），
> 不是 `data/config/` —— 我自己的验证脚本一开始就找错了地方。

**验证边界（重要）**：检测、判据、镜像表、降级顺序、通知策略、失败隔离
都有单元测试（22 个），并且用**真实挪走文件**的方式端到端验过 API 返回值。
**但「真的能从网上下下来并装到位」没验过** —— 开发机连不上外网。
详见 §8 的未验证表。

### ✅ P0：daemon 的真实时段验证 —— 已完成（`08cec6e`）

已用「覆盖当前时刻」的课表在真实时间轴上跑通：

```
[2026-09-20] 载入 1 节课，1 个悬浮窗时段，3 个处理窗口
开始录制「daemon验证课」-> ...（引擎：ffmpeg + h264_amf + WASAPI）
进入处理窗口「午休」（scope=morning）
WARN [job] 正在录制中，本轮跳过（等它录完再处理）
```

顺带**修掉一个真 bug**：录制一开始就把作业标成 `Recorded` 存盘，
于是它立刻出现在 `pending()` 里；若处理窗口与上课时段重叠，
**还没写完的录像会被捞去处理**，报「转写失败：没有可转写的音视频文件」
并把作业标成 Failed。已让 `process_scope` 排除正在录制的作业（见 §5 坑 #19）。

**后三步也补完了**（`--process-only` 模式：处理窗口开到现在、上课时段挪到已过去、
保留 `data/` 重跑 daemon。脚本是 `_tmp/daemon_prep.py --process-only`）：

```
进入处理窗口「午休」（scope=morning）
待处理作业 1 个
[..._daemon验证课_default] 转写完成（114 字 / 23 段 / whisper.cpp(tiny) / 14.1s）
[..._daemon验证课_default] 截图关联完成（1 张）
[..._daemon验证课_default] 文档生成完成（Word=true PDF=false）
[..._daemon验证课_default] 推送成功（dry-run）
[..._daemon验证课_default] 已登记清理计划（72 小时后到期）
```

终点 `job.json`：`state=PUSHED`、`push_succeeded_at` 有值、`expire_at=+72h`、`last_error=null`；
产物 Word 25 KB + HTML + Markdown。

> **这段验证本身踩了个坑**：第一次让处理窗口开到现在时，daemon **一声不吭、
> 什么都没做**。原因见坑 #20 —— 我为了阻止它再录一节新课，把课表时段改到了
> 「已经过去」，而 `job_id` 是用**当前课表**反算的，跟一天前录好的作业对不上。
> `--process-only` 现在改为从作业的 `job.json` 里读回真实时段来生成课表条目。

**仍需人工观察**：真实课表下连着跑一整天（多节课、多个窗口、跨午休），
以及 PDF 出图、真实 QQ/微信推送、悬浮窗与托盘观感。

### ✅ P1：带参数的文案迁移 —— 已完成（`cec12d5`）

`cmd.rs` 里最后 108 处带插值的文案已全部外置；加上早先的 79 处，
**CLI 的固定输出字符已经全部来自语言文件**。

- 迁移器 `_tmp/i18n_migrate2.py`（默认 dry-run，加 `--apply` 才动文件）
- 做法：括号匹配取调用体 → 取字面量与剩余实参 → 按顶层逗号切实参
  （跳过字符串内部的括号与逗号）→ `{}` 依次命名成 `{p1}`、`{name}` 内联捕获保持原名
- **刻意跳过**（脚本会逐条列出，别用正则硬来）：纯占位符 `"{}"`、
  含 `{:>2}` / `{:<20}` / `{:.1}` 这类宽度规格的、含 `{{` 或 `\n` 的。
  这些留在代码里反而更清楚。
- **务必用全路径 `vca_core::i18n::tf(...)`**，短名会撞上源码里的局部变量（见坑 #13）
- 迁移后 clippy 冒出 14 处 `unnecessary_to_owned`（脚本无脑加 `.to_string()`，
  而有些实参本来就是 `String` / `&str`），`cargo clippy --fix` 一把修掉
- key 走 gettext 风格（中文原文即 key）：代价是 key 长，
  好处是漏翻时界面显示原文，而不是一串看不明白的下划线

顺带修掉两处「中文藏在代码里当实参」的漏网之鱼：单双周的 `"单"/"双"`、
教师名单的顿号分隔符 —— 切英文界面时它们会原样冒出中文，现走
`cmd.word.odd/even` 与 `cmd.list_sep`。

### ⏸ P2：9 个内置插件本体 —— **用户已确认可暂缓**

`plugins/` 下 9 个目录只有 manifest + README，`entry` 指向不存在的可执行体
（路径格式已修正为 `bin/xxx.exe`）。

**用户的判断是：这些内置插件对实际体验影响不大，可以先不写。**
（宿主可用、范例可抄，功能上内置实现已经在跑，插件只是"可替换"的接口形态。）

真要做时：`plugins/example-echo/` 是**可抄的完整范例**——

```bash
cargo build --release -p vca-plugin-example
cp target/release/vca-plugin-example.exe plugins/example-echo/bin/
```

三个易踩的规矩写在 `example-echo/README.md` 里（stdout 只能走协议、凭据走环境变量、api_version 大版本要对）。

### ✅ P3：需求外功能 `overlay` —— **用户已确认保留**

`overlay.rs`（1000+ 行）+ `tray.rs` 是上一任 agent 自创的（原始需求没提过桌面悬浮窗），
但**用户明确表示保留**。

所以现状就是最终形态：`vca overlay demo/plan` 命令可用，托盘图标可用。
**不要再提议删除它。**

> 仍是待验证项：悬浮窗与托盘在真实一体机上的显示效果（我这里只能验证"创建成功"，
> 看到的效果需要人眼确认）。

---

## 9.1 本轮验证时发现的其他改进点（未做，供参考）

1. **「载入 0 节课」对用户不友好**：单双周过滤是正确行为，但用户只看到
   「载入 0 节课」，不知道是被单双周滤掉的。建议在课程表有条目却载入 0 节时，
   明确提示「今天有 N 条课表条目，但都不在本周周期内」。
2. **处理窗口与上课时段重叠没有告警**：目前只在跳过时打一条 WARN。
   更好的做法是在启动时检查并提示「处理窗口与上课时段重叠，建议改成休息段」。
3. **处理窗口匹配不上作业时是静默的**：`process_scope` 里 `todo.is_empty()`
   直接 `return Ok(())`。建议在 `pending()` 非空、`todo` 为空时打一条
   INFO/WARN ——「本窗口有 N 个待办作业，但都不属于本轮 scope 的课表条目」，
   否则用户看到的就是「处理窗口没反应」（坑 #20 就是这么被发现的）。
4. **语言文件的 key 是新旧两套风格**：最早那 79 条的 key 是**截断**的中文
   （`cmd.out.说明SKIP表示该项缺失也不`），读起来像半句话；后来的 108 条
   统一用完整原文作 key。要统一的话写个脚本按 value 反推完整 key 即可，
   纯机械重构，风险低但很啰嗦 —— 不影响功能，优先级低于真机观察项。


---

## 10. 常用命令速查

```powershell
# 构建与运行
cd D:\VibeClassAgent
.\scripts\dev.cmd                       # 编译并进交互界面（数字菜单 + / 命令）
.\scripts\dev.cmd -Check                # 只做 cargo check
.\scripts\dev.cmd doctor                # 环境自检
.\scripts\dev.cmd plugin list           # 10 个插件
.\scripts\dev.cmd debug e2e             # 端到端冒烟（录 15 秒跑完整流程）
.\scripts\dev.cmd debug audio           # WASAPI 端点探测
.\scripts\dev.cmd debug record-test     # 录 10 秒（含截图与合并）
.\scripts\dev.cmd debug transcribe      # 对上次录制跑转写

# 依赖与打包
python scripts/fetch-deps.py            # 下载 ffmpeg / whisper / 模型
python scripts/fetch-deps.py --models base
python scripts/package.py               # 打包（自带运行时，约 186 MB）
python scripts/make-icon.py             # 重新生成占位图标

# 质量门禁
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
```

**交互界面里的命令**：

| 数字 | 功能 |
|---|---|
| 1 | 首次引导向导 |
| 2 | 环境自检与自动修复 |
| 3 | 录制测试 |
| 4 | 处理待办作业 |
| 5 | 查看状态与作业 |
| 6 / 7 | 试运行 / 正式运行守护进程 |
| 8 | 预览录像清理 |
| 0 | 退出 |

| 斜杠命令 | 用途（开发者） |
|---|---|
| `/status` `/jobs` | 状态与作业列表 |
| `/process [dry]` | 跑课后流水线 |
| `/model` | 拉取模型列表并选择 |
| `/paths [set]` | 查看/修改数据目录 |
| `/doctor [fix]` `/config` | 自检与配置 |
| `/audio` `/record-test` `/transcribe-test` `/e2e` | 诊断探针 |
| `/clean` `/logs` `/clear` `/quit` | 清理、日志、清屏、退出 |

---

## 11. 关键环境变量

| 变量 | 用途 |
|---|---|
| `VCA_LLM_API_KEY` | 要点提取的模型 Key |
| `VCA_STT_API_KEY` | 云端转写 Key（用本地转写则不需要） |
| `VCA_PUSH_TOKEN` / `VCA_PUSH_ENDPOINT` | 推送令牌与地址 |
| `VCA_QQ_APP_ID` / `VCA_QQ_APP_SECRET` | QQ 官方机器人凭据 |
| `VCA_FFMPEG` / `VCA_WHISPER` / `VCA_WHISPER_MODEL` | 覆盖外部组件路径 |
| `VCA_TOOLS_DIR` | 覆盖 `tools/` 目录 |
| `VCA_DATA_DIR` / `VCA_CONFIG_DIR` | 覆盖数据/配置目录 |
| `VCA_PROXY` | HTTP 代理 |
| `VCA_BROWSER` / `VCA_ICON` / `VCA_LOCALE_DIR` | 浏览器 / 图标 / 语言文件路径 |

密钥也可以放 `<配置目录>/secrets.env`（首次引导里填的 Key 就写在这里）。
规则是**只补缺失的环境变量**——显式设过的环境变量不会被文件悄悄盖掉。

---

## 12. 提交脉络（最近的在前）

> **本轮改动尚未提交**（工作区里是脏的）。已完成的五项见 §9 P0.5 与此处说明：
> ① 启动慢根治（单实例保护 + ui-profile 从 `%TEMP%` 移到 `%LOCALAPPDATA%`）；
> ② 托盘左键点开界面、右键菜单动态显示状态（是否在录制／守护进程是否在跑／
> 待处理作业数／已运行时长／最近一次错误）；
> ③ 关掉一切"浏览器套壳"痕迹（F12、Ctrl+Shift+I/J/C、Ctrl+U、右键菜单）；
> ④ 启动即自检依赖缺失并提供一键下载（§9 P0.5）；
> ⑤ **启动耗时定位**（§4.8）—— 新增 `VCA_STARTUP_TRACE=1` 分段计时，
> 实测自身只占 74 ms；并加了 Edge profile 的后台预热。
> 三关全绿：fmt / clippy 零告警 / **379 测试**；smoke 58 项全过。

| 提交 | 内容 |
|---|---|
| `9a11b9b` | **推送渠道裁剪到三类 + 微信折叠进二级菜单 + 课表模型对齐 ClassIsland**（见 §8.5、§12 下方说明） |
| `55cbac2` | smoke 补 QQ 官方机器人的凭据缺失检查（共 54 项） |
| `c5d32b8` | smoke 覆盖异常与重试 + 参考项目分析文档 |
| `cec12d5` | 剩余 108 处带参数文案外置、英文包补齐、单双周与名单分隔符也走语言文件 |
| `08cec6e` | daemon 修复：正在录制的作业被当成待办捞走（坑 #19） |
| `fade908` | 数字菜单并入主界面、删 tagline、命令归开发者区、79 处文案迁移 |
| `2a2b07e` | 修正 README 调用示例（PowerShell 的 `.\` 前缀） |
| `e69bac9` | 插件架构落地：第一个功能完整的插件 + 修 recorder id 撞名 |
| `3b7ca0a` | 代码 review 收尾：clippy 归零、fmt 无差异 |
| `8b77fe7` | 修 `dev.cmd` 无参数报错（`$Args` 是 PowerShell 自动变量） |
| `134a9df` | 托盘图标加载的静默故障测试 |
| `05bcc0d` | setup 接入语言文件、打包带资源与语言目录、README 补定制章节 |
| `ba8ff04` | ASCII 字符画 + Claude Code 配色 + 全文案多语言化 + 模型自动拉取 + secrets.env |
| `54285b5` | 端到端冒烟 + README 重写 |
| `2f42a2c` | Claude Code 风格交互界面 |
| `7cbd3b7` | 流水线改纯 Rust（转写/提取/截图关联/文档/推送） |
| `35b8bdb` | 三模式 LLM + 浏览器自动化 + 文档生成 + QQ 双通道 |
| `75a0205` | HTTP 换 ureq + 本地转写打通 |
| `94e2805` | 录制链路端到端打通（WASAPI + 编码器实测） |
| `42a1230` | 打通 ureq/rustls/ring 依赖链 + 修 dev.ps1 编码 |
| `b7053fa` | WASAPI 系统声音采集 + raw-dylib 链接 |
| `18b5724` | **接手前的存档快照**（原 agent 交付状态，未改动任何源码） |

---

## 13. 一句话总结

**这个项目从"一堆 4.2 GB 的半成品垃圾"变成了一个能跑的纯 Rust 程序**：
四大核心功能实测通过、373 个测试全绿、体积从 4.2 GB 降到 190 MB。
daemon 已在真实时间轴上跑完「到点开录 → 录制中拒绝处理 → 收尾 → 处理窗口内
自动跑完流水线 → 作业 `PUSHED` → 登记 72 小时清理」；CLI 文案 100% 外置
（zh-CN / en-US 各 328 个 key，双向差集为空）。

交付形态也补齐了「新手拿过去能不能用」这一环：**单实例保护**（重复双击不会开一堆）、
**启动即自检依赖**（缺 ffmpeg / whisper / 模型会弹窗说明后果并一键下载，
必需项每次都提示、可选项只提示一次），以及**托盘与界面的正常 Windows 行为**
（左键点开界面、右键看状态、无 F12 / 无右键菜单 / 无地址栏）。

剩下的活只剩两类：**9 个内置插件本体（用户已确认可暂缓）**，
以及只能靠人眼与真机确认的观察项 —— PDF 出图、真实 QQ/微信推送、
悬浮窗与托盘观感、在一体机上连跑一周、**依赖的真实下载**（开发机无外网）。

接手时请**先跑一遍再做判断**，不要凭这份文档或 README 里的描述下结论 ——
上一任 agent 的教训就是文档与实现严重脱节。
