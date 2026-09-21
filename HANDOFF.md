# VibeClassAgent 交接文档（HANDOFF）

> **给接手的 AI / 开发者**：这份文档的目的是让你**在 10 分钟内接上工作**，
> 而不是像第一轮那样从头读一遍代码再猜上下文。
> 里面每一条都基于实际做过的事和实测结果；不确定的地方明确标注了「待确认」。
>
> 最后更新：2026-09-19 ｜ 对应提交 `fade908`

---

## 0. 先读这一节（最重要）

**这个项目已经能跑，不要再从头重构。**

- 代码 **206 个单元测试全绿**，`cargo clippy --workspace --all-targets` **零告警**，`cargo fmt` **无差异**。
- 四大核心功能（静默录制 / 课后处理 / 推送 / 清理）**已端到端验证通过**。
- 上一任 agent 留下的 4.2 GB 垃圾（Python 运行时、工具链、构建产物）已清理，
  现在是纯 Rust，约 190 MB（含 ffmpeg + whisper 模型）。

**接手后第一件事**：跑 `scripts\dev.cmd doctor` 和 `scripts\dev.cmd debug e2e`，
确认真实状态与你读到的一致，再动代码。

**三条硬规矩**（违反会直接破坏用户信任）：

1. **不要在 README / 文档里写没验证过的数字**。上一任 agent 的 README 同一页里写着
   「109 个测试」和「88 个测试」，实测是 123——这就是用户说"太垃圾了"的直接原因之一。
2. **不要提交凭据**。所有 token 走环境变量或 `secrets.env`（已被 .gitignore 排除）。
3. **改动必须过三关再提交**：`cargo fmt --all` → `cargo clippy --all-targets` → `cargo test --workspace`。

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
| release 二进制 | **5.52 MB** |
| 端到端 | 录 15 秒 → 转写 → 截图关联 → Word(626 KB) → 推送 → 登记 72h 清理，全绿 |

---

## 3. 代码地图

```
D:\VibeClassAgent\
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
- 已经投入 1.2 万行 + 206 个测试 + 三处工具链适配，换语言是负收益。

C++ 只在"进程内自研音视频流水线"上更强，而本项目选择了「ffmpeg 子进程 + 编排层」，
恰好绕开了那个区域。

### 4.2 去 Python 化

上一任用 Python 做算法侧（转写/文档/去重），拖进 263 MB 的 vendor。
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

### 4.7 便携布局 + 引导文件

数据默认放在**程序所在目录**（`data/` 与 `config/`），不碰 C 盘用户目录。
用户可用 `/paths set D:\课堂数据` 换位置，选择记在 exe 同级的 `vca.paths.json`
（不能记在待选择的目录里，那是鸡生蛋问题）。

优先级：`--data-dir` > `VCA_DATA_DIR` > 引导文件 > 便携 > `%LOCALAPPDATA%` 兜底。

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
- `python/`：**已被去 Python 化废弃**（vendor 263 MB 仍在磁盘上，不参与构建）。
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
- **质量关（本轮）**：`scripts\gate.cmd` 三关全过 —— fmt 合规、clippy（`--workspace --all-targets -- -D warnings`）零告警、256 个单元测试全绿
- **GUI 形态（本轮）**：`python scripts/smoke.py` 41 项全过 ——
  静态资源、令牌保护（错误 token 403）、配置写入与回读、真实发送测试消息
  （自带 mock OneBot 收报文并校验内容）、时间表推导处理窗口、
  ClassIsland 课表导入、逐条「录制」开关落盘、界面文案接口、
  机器人探测、守护进程起停、作业与清理预览、退出接口
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

---

## 9. 未完成事项与优先级

按价值排序（**建议按这个顺序做**）：

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

| 提交 | 内容 |
|---|---|
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
四大核心功能实测通过、206 个测试全绿、体积从 4.2 GB 降到 190 MB。
daemon 已在真实时间轴上跑完「到点开录 → 录制中拒绝处理 → 收尾 → 处理窗口内
自动跑完流水线 → 作业 `PUSHED` → 登记 72 小时清理」；CLI 文案 100% 外置
（zh-CN / en-US 各 328 个 key，双向差集为空）。

剩下的活只剩两类：**9 个内置插件本体（用户已确认可暂缓）**，
以及只能靠人眼与真机确认的观察项 —— PDF 出图、真实 QQ/微信推送、
悬浮窗与托盘观感、在一体机上连跑一周。

接手时请**先跑一遍再做判断**，不要凭这份文档或 README 里的描述下结论 ——
上一任 agent 的教训就是文档与实现严重脱节。
