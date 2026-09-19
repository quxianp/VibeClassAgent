# VibeClassAgent 项目说明（代码维护与使用报告）

> 版本：v0.2　|　技术栈：Rust（宿主）+ Python（算法侧）　|　平台：Windows 10+
> 状态：**功能完整的可运行系统**，88 个单元测试通过、clippy 零告警
> 配套文档：《如何复刻》《Rust 上手文档》《课程表与时间表编辑文档》

---

## 目录

1. [项目是什么](#1-项目是什么)
2. [技术栈与选型理由](#2-技术栈与选型理由)
3. [目录结构与逐文件职责](#3-目录结构与逐文件职责)
4. [架构与数据流](#4-架构与数据流)
5. [各功能实现方式](#5-各功能实现方式)
6. [关键算法与逻辑](#6-关键算法与逻辑)
7. [使用指南（CLI 全集）](#7-使用指南cli-全集)
8. [维护指南](#8-维护指南)
9. [测试与质量](#9-测试与质量)
10. [已知限制与待办](#10-已知限制与待办)
11. [安全规范](#11-安全规范)
12. [附录：快速索引](#12-附录快速索引)

---

## 1. 项目是什么

面向**希沃一体机**（多教师共用、日常可能卡顿）的**静默课堂录制与课后总结系统**。

### 1.1 完整业务流程

```
时间表 + 课程表
      
        到点自动开始（无窗口、无弹窗、无托盘打扰）
   
     静默录屏录音    720p@8fps + 每 180 秒抓一张截图
   
            下课后 5 分钟
   
     桌面悬浮窗      「Agent Overrr :)」玻璃小窗，可拖动
     上课前 2 分钟自动关闭
            进入午休 / 放学后 / 晚自习后窗口
   
     课后处理流水线                        
      语音转写（本地 faster-whisper）      
      要点提取（用户自填的 LLM API）       
      截图去重关联（pHash）               
      生成 Word + PDF（本地）             
      推送到微信 / QQ                     
   
            推送成功时刻起算
   
     72 小时后删除   推送未成功则**永不删除**
   
```

### 1.2 六个硬性设计约束（全部来自实际需求）

| # | 约束 | 实现方式 |
|---|---|---|
| 1 | 机器本身就卡，不能再加重负担 | 极简档默认（720p@8fps/CRF30/tiny 模型）；负载自适应降档；转写错峰到休息窗口；Python 子进程用完即退 |
| 2 | 完全静默，不能有弹窗 | 所有子进程 `CREATE_NO_WINDOW`；崩溃时 `SetErrorMode` 屏蔽系统对话框；仅一个小托盘图标 |
| 3 | 多教师共用一台机器 | 以 profile 隔离数据/配置/密钥/推送目标 |
| 4 | 录屏绝不上云 | 转写纯本地；只有**转写后的文本**会发给用户自己填的 LLM |
| 5 | 推送成功才允许删除 | 删除条件为「`pushed == true` **且** 已到期」，算法层面杜绝误删 |
| 6 | 不能预置任何学校数据 | 出厂时间表/课表为空；示例一律用「课程一/教师甲」等虚构值 |

---

## 2. 技术栈与选型理由

| 层 | 选型 | 为什么 |
|---|---|---|
| 宿主 / 调度 / CLI / 插件管理 | **Rust** | 体积小、内存低、并发安全；Cargo 统一依赖，长期无人专职维护时更省心 |
| 重算法（转写 / 文档 / 去重） | **Python 子进程** | 生态无可替代；**按需拉起、用完释放**，规避其内存大、启动慢 |
| 录制引擎 | **ffmpeg**（gdigrab + dshow） | 兼容性好、可控、无需额外运行时 |
| 语音转写 | **faster-whisper**（本地） | 纯离线、零调用费、不上云 |
| 文档导出 | **python-docx**  **LibreOffice** | Word 必出，PDF 尽力（缺 LibreOffice 时降级而非失败） |
| 插件通信 | **JSON-RPC 2.0 over stdio** | 语言无关，天然进程隔离，任何语言都能写插件 |
| HTTP 客户端 | **自研 WinHTTP 封装** | 系统自带、原生 TLS，**零第三方依赖**，不再引入一个异步运行时依赖链 |
| CLI 解析 | **clap** | Rust 生态标准 |
| 配置格式 | **YAML**（serde_yaml） | 教师可手改 |
| 时间处理 | **自研** `vca-core/src/time.rs` | 刻意不引 chrono（见 10.1） |

---

## 3. 目录结构与逐文件职责

```
D:\VibeClassAgent\
 Cargo.toml              workspace 根：成员、统一依赖、release 优化
 Cargo.lock              依赖精确锁定（应用类项目应入库）
 README.md               快速上手
 .env.example            环境变量模板（**只有占位符**）
 .gitignore              排除构建产物/密钥/运行时数据/本地工具链
 .editorconfig           统一缩进换行

 .vscode/                VSCode 项目配置（详见 8.4）
 PlanningDocs/           策划书与全部文档
 config/                 配置模板（settings/timetable/schedule）
 scripts/                dev.cmd / dev.ps1 / doctor.*
 plugins/                9 个插件占位
 python/                 算法侧 Worker
 crates/                 6 个 Rust crate
```

### 3.1 `crates/vca-core/`  领域核心（**无 IO、无 unsafe、纯逻辑**）

| 文件 | 行数级 | 职责 |
|---|---|---|
| `lib.rs` | 40 | 模块声明；`VERSION`；`PRODUCT_NAME`（改名改这里）；`DEFAULT_DATA_DIR_NAME` |
| `time.rs` | 260 | **零依赖**日期时间：civil 日期算法、星期推算、ISO 周键、跨日加减、`HH:mm` 解析、serde 字符串读写。**6 个测试** |
| `schedule.rs` | 520 | **排课计划**：星期过滤、单双周（支持条目级）、周末模板、**连堂合并**、停课调课、导入预校验、**悬浮窗时序**。**13 个测试** |
| `model.rs` | 330 | 领域模型：时间表/课程表/教师/处理窗口/录像/转写片段/要点/`Override`/`OverlaySettings`/颜色解析 |
| `config.rs` | 330 | 配置结构 + YAML 加载/校验 + 性能预设；`TimetableFile`/`ScheduleFile`/`WeekendTemplate` |
| `cleanup.rs` | 260 | **72 小时清理**：`DeletePlan` 计划文件、到期判定、目录扫描删除、dry-run。**5 个测试** |
| `job.rs` | 130 | **作业状态机**：`PendingPushedCleaned`，含 `Failed` 重试与合法性校验 |
| `store.rs` | 250 | **作业持久化**：原子写 JSON、幂等创建、按状态查询。**5 个测试** |
| `import.rs` | 700 | **ClassIsland JSON 导入** + **CSV 导入** + 教师名清洗 + 存档。**10 个测试** |
| `windows.rs` | 210 | **处理窗口推导**：从时间表休息段自动生成。**6 个测试** |
| `paths.rs` | 110 | 目录布局与命名规则（profile 隔离、存档路径） |
| `error.rs` | 40 | 统一错误类型 |

### 3.2 `crates/vca-ipc/`  插件通信契约

| 文件 | 职责 |
|---|---|
| `lib.rs` | JSON-RPC 2.0 消息结构（`Request`/`Response`/`RpcError`）、5 个方法名常量、7 类插件的载荷结构（`RecordResult`/`TranscriptResult`/`DocResult`/`PushResult`/`CleanResult`）。**8 个测试** |

### 3.3 `crates/vca-platform/`  平台适配（**全项目唯一允许 unsafe 的地方**）

| 文件 | 职责 | 关键点 |
|---|---|---|
| `overlay.rs` | **桌面悬浮窗** | Win32 窗口 + **分层窗口逐像素 Alpha** + 44 超采样抗锯齿圆角 + 液态玻璃渐变 + 系统主机色 + 拖动。**3 个测试** |
| `crash.rs` | **崩溃静默退出** | `SetErrorMode` 关弹窗 + panic 钩子写日志 + `exit(70)` + `guard()`。**2 个测试** |
| `http.rs` | **HTTP 客户端** | WinHTTP FFI；URL 解析、multipart、urlencode。**7 个测试** |
| `push.rs` | **推送通道** | trait + 5 个实现（企业微信/通用 Webhook/Server 酱/第三方/dry-run）。**8 个测试** |
| `llm.rs` | **要点提取** | OpenAI 兼容协议、JSON 约束、**超长文本分段**、失败降级。**8 个测试** |
| `capture.rs` | **录制引擎** | ffmpeg 参数拼装、`CREATE_NO_WINDOW`、`q` 优雅停止、负载降档。**5 个测试** |
| `screenshot.rs` | **屏幕截图** | GDI BitBlt  24 位 BMP（无编码库）。**2 个测试** |
| `clock.rs` | 本机时钟 | `GetLocalTime` FFI；`now_local()` / `now_local_secs()`。**1 个测试** |
| `probe.rs` | 负载探测 | CPU/内存/磁盘 + `LoadLevel` 三级 |
| `session.rs` | 运行会话判定 | 计划任务 vs 服务的可采集性判定 |
| `doctor.rs` | 环境自检 | ffmpeg / Python / LibreOffice 存在性 |

### 3.4 `crates/vca-plugin-host/`  插件宿主

| 文件 | 职责 | 测试 |
|---|---|---|
| `manifest.rs` | `manifest.toml` 结构、接口版本校验、风险确认判定 |  |
| `host.rs` | **插件发现**：扫描目录、解析清单、校验、坏插件跳过不拖垮宿主 |  |
| `runner.rs` | **插件进程运行时**：拉起子进程 + JSON-RPC over stdio + 超时 | **4 个** |
| `market.rs` | **本地市场**：inspect / install（含权限与风险清单）/ uninstall | **5 个** |

### 3.5 `crates/vca-engine/`  编排引擎

| 文件 | 职责 | 测试 |
|---|---|---|
| `pipeline.rs` | **课后处理流水线**：五步推进、每步落盘、失败不丢数据、缺 LLM 时降级 | **4 个** |
| `daemon.rs` | **调度守护进程**：录制联动、截图采集、悬浮窗驱动、处理窗口触发、定期清理 | **3 个** |
| `worker.rs` | Python Worker 的 stdio JSON-RPC 客户端 | **1 个** |

### 3.6 `crates/vca-cli/`  命令行入口

| 文件 | 职责 |
|---|---|
| `main.rs` | clap 命令树、日志初始化、目录布局构造、子命令分发 |
| `cmd.rs` | 各子命令实现（`doctor` / `config` / `overlay` / `run` / `clean` / `plugin` / `import` / `debug`） |

### 3.7 `python/vca_worker/`  算法侧

| 文件 | 职责 |
|---|---|
| `rpc.py` | JSON-RPC over stdio 最小实现；**强制 UTF-8**（Windows 管道默认 GBK） |
| `__main__.py` | 入口与 `run` 分发（`transcribe` / `docgen` / `dedupe`）+ `--selftest` |
| `transcriber.py` | faster-whisper 转写；长音频切片；模型进程内缓存 |
| `docgen.py` | python-docx 生成 Word；LibreOffice 转 PDF（缺失则降级） |
| `linker.py` | pHash 截图去重；缺依赖时退化为均匀抽样（**永不失败**） |

### 3.8 `plugins/`  9 个插件占位

`recorder` / `transcriber` / `extractor` / `screenshot-linker` / `doc-generator` /
`pusher-wecom` / `pusher-qq` / `pusher-wechat-personal`（带 `risk_note`）/ `cleaner`

每个含 `manifest.toml` + `README.md`。

---

## 4. 架构与数据流

### 4.1 分层

```

 交互层   vca-cli（clap 命令树） 配置  日志  托盘图标        

 编排层   vca-engine                                          
    daemon（调度循环） pipeline（课后流水线） worker（RPC） 

 插件层   vca-plugin-host（发现/运行时/本地市场）+ plugins/    

 核心层   vca-core（模型/时间/排课/清理/导入/持久化）          
          vca-ipc （协议契约）                                

 平台层   vca-platform（Win32 FFI：窗口/截图/时钟/HTTP/推送）  

```

### 4.2 依赖方向（**必须保持单向**）

```
vca-cli > vca-core        （纯逻辑，可脱离 Windows 单测）
          > vca-ipc
          > vca-plugin-host
          > vca-platform
          > vca-engine > vca-core
                            > vca-ipc
                            > vca-platform
                            > vca-plugin-host
```

`vca-core` 与 `vca-ipc` **不反向依赖任何上层**这是它们能保持零 unsafe、纯逻辑的原因。

### 4.3 运行时数据流

```
 调度：vca-engine::daemon
     每小时/每事件唤醒  lessons_today()  判断该录哪节
                                         判断悬浮窗该显该隐
                                         判断是否进入处理窗口

 录制：vca-platform::capture
     到点  CaptureSession::start()（ffmpeg 无窗口子进程）
     期间  screenshot::capture_timed() 每 180 秒
     到点  CaptureSession::stop()（先发 'q' 优雅收尾）

 处理：vca-engine::pipeline（在休息窗口内串行执行）
     Recorded     WorkerClient> 转写文本
     Transcribed  llm::extract_summary> 要点 JSON
     Extracted    link_screenshots> 精选截图
     Linked       WorkerClient> Word + PDF
     DocReady     push::Pusher> 推送成功  记 expire_at

 清理：vca-core::cleanup
     每 30 分钟扫描一次  判定 is_expired  删除  删计划文件

 展示：vca-platform::overlay
     独立线程消息循环；PostMessage(WM_APP+1/+2) 控制显隐
```

### 4.4 作业目录结构

```
data/profiles/<profile>/
 jobs/<jobId>/
    job.json          作业状态（原子写）
    transcript.txt    转写结果
    summary.json      要点
 raw/<yyyyMMdd>/*.mp4  原始录像
 audio/<yyyyMMdd>/*.wav 音频
 screenshots/<yyyyMMdd>/*.bmp
 docs/<yyyyMMdd>/*.docx|.pdf
 trash/                （软删除区，保留 72 小时）
 logs/crash.log        崩溃日志
```

---

## 5. 各功能实现方式

### 5.1 静默录制（`vca-platform::capture`）

**ffmpeg 参数**（`build_args()`）：

```
-hide_banner -loglevel error -y
-f gdigrab -framerate 8 -i desktop           屏幕
-f dshow -i audio=...                         音频
-c:a aac -b:a 64k -ac 1
-vf scale=-2:720 -c:v libx264 -preset veryfast -crf 30 -pix_fmt yuv420p
output.mp4
```

**静默性**：`CREATE_NO_WINDOW`（`std::os::windows::process::CommandExt::creation_flags`）。

**优雅停止**：向 ffmpeg 的 stdin 写 `q`，它会把 moov 原子写好再退出；
最多等 5 秒，超时才 `kill()`**避免录像文件损坏**。

**内存安全**：`CaptureSession` 实现了 `Drop`，进程退出时自动停掉子进程，不留孤儿。

**自适应降档**（`CaptureParams::degrade`）：

| 负载等级 | 动作 |
|---|---|
| Low | 不变 |
| Medium | fps 减半（不低于 5），CRF +2（不超过 35） |
| High | fps=5，CRF=35，分辨率降到 480p |

### 5.2 桌面悬浮窗（`vca-platform::overlay`）

**需求**：下课后 5 分钟在右上角显示小窗，文字 `Agent Overrr :)`，上课前 2 分钟关闭，可拖动。

**窗口属性**：

| 需求 | 手段 |
|---|---|
| 无边框无标题栏 | `WS_POPUP` |
| 始终置顶 | `WS_EX_TOPMOST` + `SetWindowPos(HWND_TOPMOST, SWP_NOACTIVATE)` |
| **不抢焦点**（不打断教师操作） | `WS_EX_NOACTIVATE` + `ShowWindow(SW_SHOWNOACTIVATE)` |
| 不出现在任务栏 | `WS_EX_TOOLWINDOW` |
| 半透明 | `WS_EX_LAYERED` + `UpdateLayeredWindow`（逐像素 Alpha） |

**渲染管线**（`render_layered()`）这是**踩坑后重写的正确方案**：

1. 建 32 位 DIB；
2. **直接在原始像素缓冲上写预乘 ARGB**：逐行算渐变底色；
3. 圆角用**有符号距离场 + 44 超采样**求覆盖率  8 位平滑过渡；
4. 顶部加 1px 高光条（玻璃反光感）；
5. 文字：先用 GDI 画到「黑底白字」临时 DIB，再按 `覆盖率 = 亮度` 反解，
   在 Rust 里做 over 合成  **文字边缘也抗锯齿**；
6. `UpdateLayeredWindow` 一次性提交，DWM 按 Alpha 与桌面合成。

**踩坑记录**（详见《如何复刻》7.3）：

| 错误做法 | 后果 |
|---|---|
| `SetWindowCompositionAttribute` + `ACCENT_ENABLE_ACRYLICBLURBEHIND` | DWM 接管背景，把本进程 GDI 绘制的**文字与圆角全部覆盖**  整窗纯色无字 |
| `SetWindowRgn` 做圆角 | 区域是 **1 位掩码**，物理上不可能抗锯齿 |
| `DrawTextW` 前用 `GetClientRect` | 一旦失败返回全 0 矩形  有底无字 |

**时序计算**（`vca-core::schedule::overlay_windows`）：

```
对当天每节课 i：
  show_at = end_i + show_delay_minutes          // 默认 +5 分钟
  next    = 之后开始时刻 > end_i 的最早一节课
  hide_at = next ? next.start - hide_before_minutes   // 默认 -2 分钟
                 : show_at + fallback_minutes         // 无后续课程兜底 30 分钟
  若 hide_at <= show_at  丢弃（课间太短，避免闪现）
```

**拖动**：`WM_LBUTTONDOWN` 记录光标与窗口位置  `SetCapture`；
`WM_MOUSEMOVE` 算差值 `SetWindowPos` 并重新提交；
`WM_LBUTTONUP`/`WM_CAPTURECHANGED` 释放。

### 5.3 崩溃静默退出（`vca-platform::crash`）

**需求**：「如果上课中途崩溃则静默退出」。

三层防护：

| 层 | 手段 | 作用 |
|---|---|---|
| 系统层 | `SetErrorMode(SEM_FAILCRITICALERRORS \| SEM_NOGPFAULTERRORBOX \| SEM_NOALIGNMENTFAULTEXCEPT)` | 禁止弹出「程序已停止工作」对话框 |
| 语言层 | `std::panic::set_hook` | 把 panic 的位置与原因**追加**写入 `logs/crash.log` |
| 退出 | `std::process::exit(70)` | 不打印、不弹窗；**退出码 70** 用于外部识别崩溃 |

另有 `crash::guard(&handler, "标签", || { ... })` 可包裹任意可能 panic 的代码块。

**实测证据**：

```
$ vca --data-dir <dir> debug panic
stdout/stderr : []           完全静默
退出码        : 70
crash.log     : [2026-09-19T00:50] panicked at crates/vca-cli/src/cmd.rs:245: ...
```

### 5.4 课表导入（`vca-core::import`）

**ClassIsland JSON**：

| ClassIsland 字段 | 处理 |
|---|---|
| `TimeLayouts.*.Layouts[].TimeType` | `0`  `class`（上课节）；`1`  `break`（休息段） |
| `TimeLayouts.*.Layouts[].BreakName` |  休息段名称 |
| `ClassPlans.*.TimeRule.WeekDay` | `1..7`  `Mon..Sun` |
| `ClassPlans.*.TimeRule.WeekCountDiv` | `0every`，`1odd`，`2even`（写到**条目级** `cycle`） |
| `ClassPlans.*.Classes[i].SubjectId` | 经 `Subjects` 解析出课程名与教师名 |
| `ClassPlans.*.Classes[i].IsEnabled` | `false` 的格子跳过 |
| `Classes` 与 `Layouts` 的对应 | **按下标一一对应**（只取 `TimeType==0` 的项） |

**教师名清洗**（`clean_teacher`）：空值或纯标点（`.`/`。`/`？`/`！` 等）视为未填写  归到 `unassigned`。

**实测**（本机真实数据）：

```
时间表    : 兵团二中高二11上
课程表份数: 7
课程条目  : 98
待补全教师: 早读、体育、自习、政治、通用技术、音乐
```

**安全承诺**：全程只读，不写、不改、不删 ClassIsland 任何文件。

**CSV**：手写解析器（支持引号与 `""` 转义），校验时间格式、星期拼写、结束晚于开始；
输出 UTF-8 BOM 模板（Excel 双击不乱码）。

### 5.5 课后处理流水线（`vca-engine::pipeline`）

状态推进严格遵循状态机：

```
Recorded  Transcribed  Extracted  Linked  DocReady  Pushed
```

**四条设计原则**：

1. **每完成一步立刻 `store.save(job)`**  崩溃后从最后一个成功状态继续，**不重复调用付费接口**；
2. **失败不丢数据**：任一步失败只标 `Failed`，原始录像与中间产物全部保留；
3. **推送成功才推进到 `Pushed`**  从算法层面杜绝「没推送就删录像」；
4. **可选步骤缺失时降级而非中断**：
   - 没配 LLM  用转写原文前 8 行当摘要；
   - 没音频轨  跳过转写；
   - 没 LibreOffice  只出 Word；
   - 没 imagehash  均匀抽样截图。

**失败后的续跑**（`first_incomplete_state`）：检查 `transcript.txt` / `summary.json` / `docx_path` 是否存在，
决定从哪一步继续。

### 5.6 要点提取（`vca-platform::llm`）

- 接口：**OpenAI 兼容** `{base_url}/chat/completions`，由用户自填；
- **只上传转写文本**，绝不上传音视频；
- 提示词要求**只输出 JSON**：`{"title","points","notices","keywords"}`；
- **超长文本分段**（默认 6000 字符/段），分段提取后合并去重；
- 解析容错：能处理 ```` ```json ```` 包裹与前后废话（`extract_json`）；
- 某段失败不致命，继续下一段；全部失败才报错。

### 5.7 文档生成（`python/vca_worker/docgen.py`）

- **python-docx** 生成 Word；中文字体显式设为「微软雅黑」（`w:eastAsia`），防止 PDF 转换缺字；
- 结构：标题  课堂重点（编号列表） 重点通知（项目符号） 关键词  关键截图  附转写全文；
- **PDF** 用 LibreOffice headless 转换；找不到 `soffice` 时**只出 Word 并返回说明**，不算失败；
- 截图按 `max_screenshots` 限制数量。

### 5.8 推送（`vca-platform::push`）

统一 `Pusher` trait，5 个实现：

| 渠道 | 实现方式 |
|---|---|
| **企业微信机器人** | 先 `upload_media` 拿 `media_id`，再 `msgtype=file` 发送；上传失败降级为 markdown 文本 |
| 通用 Webhook | POST JSON（标题/摘要/附件路径） |
| Server 酱 | 表单 POST 到 `sctapi.ftqq.com/<key>.send` |
| 个人微信（第三方） | **不内置协议实现**，只把内容转发给用户自建的中转服务 |
| dry-run | 只打日志，用于联调 |

**重试**：指数退避（2s / 4s / 8s），次数由 `push.max_retries` 控制。

**凭据**：只从环境变量读（`VCA_PUSH_ENDPOINT` / `VCA_PUSH_TOKEN`），**代码与配置示例中零凭据**。

### 5.9 清理（`vca-core::cleanup`）

- 推送成功时生成 `DeletePlan`  写 `<jobId>.deleteplan.json`；
- `expire_at = push_succeeded_at + retention_hours(默认72)`；
- `sweep()` 递归扫描数据目录，收集全部计划文件；
- **删除前置条件（两者必须同时满足）**：`pushed == true` **且** `now >= expire_at`；
- 文件已不存在视为删除成功（幂等）；
- `dry_run` 只报告不删除。

### 5.10 插件机制（`vca-plugin-host`）

**清单**（`manifest.toml`）：

```toml
[plugin]
id = "pusher.wecom"
name = "企业微信机器人"
version = "1.0.0"
api_version = "1"          # 大版本不匹配  拒绝加载
kind = "pusher"            # recorder|transcriber|extractor|linker|doc|pusher|cleaner
risk_note = "..."          # 有值则安装时强制风险确认

[runtime]
type = "process"           # process | cdylib | wasm
entry = "bin/x.exe"
timeout_sec = 30

[permissions]
network = ["https://..."]
filesystem = ["read:docs/**"]
env = ["TOKEN"]
```

**运行时**（`runner.rs`）：拉起子进程  NDJSON + JSON-RPC 2.0 over stdio 
`describe` / `init` / `run` / `cleanup` / `health_check`。
同步串行调用（课后处理本身就是串行的），跳过非 JSON 日志行，按 `id` 配对响应。

**隔离性**：插件崩溃只影响自己的进程；宿主通过 `try_wait` 感知并报错。

**本地市场**（v1 仅本地）：`inspect` 校验清单与接口版本；
`install` 复制到 `plugins/<id>/` 并返回**权限与风险清单**；
已存在时拒绝覆盖（除非 `--force`）；zip 与远程来源明确返回「不支持」而非静默失败。

### 5.11 调度守护进程（`vca-engine::daemon`）

一轮循环五件事：

| 步骤 | 逻辑 |
|---|---|
|  录制联动 | 找到当前时刻落在某节 `record: true` 的课内  启动录制；离开  停止；期间按间隔抓截图 |
|  悬浮窗 | 算当天时段  `overlay_visible_at`  状态变化才调 `show()`/`hide()` |
|  处理窗口 | `windows::window_at` 命中且本日未处理过  跑该 scope 下所有待办作业（串行，作业间隔 3 秒） |
|  清理 | 每 30 分钟 `cleanup::sweep` 一次 |
|  睡眠 | 算「下一个事件时刻」精确睡眠，**上限 30 秒兜底** |

**为什么不用固定轮询**：算下一个事件时刻后睡到那时，空闲时几乎零 CPU，
同时保证不会错过上课/下课边界。

---

## 6. 关键算法与逻辑

### 6.1 civil 日期算法（`time.rs`）

把「年月日」与「距 1970-01-01 的天数」互转（Howard Hinnant 提出），不依赖任何日期库：

```
days_from_civil(y,m,d):
    y -= (m <= 2)
    era = (y >= 0 ? y : y-399) / 400
    yoe = y - era*400
    mp  = (m + 9) % 12
    doy = (153*mp + 2)/5 + d - 1
    doe = yoe*365 + yoe/4 - yoe/100 + doy
    return era*146097 + doe - 719468
```

`civil_from_days` 为其逆运算。
**星期**：1970-01-01 是周四  `weekday = (days + 3) mod 7`（0=周一）。
**闰年**：`(y%4==0 && y%100!=0) || y%400==0`。
**ISO 周键**：取该周周四所在年份与周序号（跨年周稳定），用于存档命名。

### 6.2 单双周判定

优先级：**条目级 `cycle` > 整表级 `cycle`**。

```
匹配规则：
  Every -> 任何周都匹配
  Odd   -> 仅当周序号为奇数
  Even  -> 仅当周序号为偶数
```

取值兼容 `every`/`odd`/`even`/`1`/`2`/`单周`/`双周`。

### 6.3 连堂合并（`merge_lessons`）

对已按开始时间排序的课程，相邻两项满足**全部四条**时合并：

1. 课程名相同；
2. 任课教师相同；
3. 后一节开始时间  前一节结束时间（不倒序）；
4. 间隔  5 分钟。

合并后：结束时间取后者、`record` 取逻辑或、分配同一个 `merge_group` 编号。

### 6.4 悬浮窗时序（`overlay_windows`）

见 [5.2](#52-桌面悬浮窗vca-platformoverlay) 的伪码。
另有 `overlay_visible_at(windows, now)` 判断某时刻是否该显示，
`next_transition(windows, now)` 给出下一个状态变化点（供守护进程精确睡眠）。

### 6.5 处理窗口推导（`windows.rs`）

```
遍历时间表：
  只取 type == break 的段
  时长 >= min_minutes（默认 15）才作为窗口
  scope 由开始时间决定：<12:00 morning；12:00-18:00 afternoon；否则 evening
  窗口默认串行（max_concurrent = 1）
```

**为什么从休息段推导**：各校作息差异极大，硬编码「午休 12:10」必然不适配；
而时间表里已经标好了休息段，直接复用既准确又零配置。

### 6.6 72 小时保留

```
expire_at = push_succeeded_at + retention_hours(默认 72)
删除条件 = (pushed == true) AND (now >= expire_at)
```

**关键**：`pushed == false` 时 `is_expired()` **恒为 false**
即使 `expire_at` 被错误设置为过去时间，也绝不删除。这是**算法层面**的安全保证，不依赖调用方自觉。

### 6.7 作业状态机

```
Pending  Recorded  Transcribed  Extracted  Linked  DocReady  Pushed  Cleaned
                  任意状态  Failed  Recorded（重试）
```

`JobState::can_transition_to()` 用 `matches!` 显式列出所有合法迁移对；
`transition_to()` 非法时返回 `CoreError::InvalidTransition`。

### 6.8 圆角抗锯齿（`overlay.rs`）

```
rounded_sdf(px, py, w, h, r):
    qx = |px - w/2| - (w/2 - r)
    qy = |py - h/2| - (h/2 - r)
    return length(max(q,0)) + min(max(qx,qy),0) - r      // 负值 = 在内部

corner_coverage(x, y, w, h, r):
    对像素内 44 = 16 个采样点，统计 sdf <= 0 的比例
```

得到的覆盖率直接作为该像素的 Alpha，**边缘得到 8 位平滑过渡**。

### 6.9 文字抗锯齿的「反解」技巧

GDI 在 32 位 DIB 上绘制不会正确设置 Alpha 通道。解法：

1. 建一张临时 DIB，**填纯黑**；
2. 用 `SetTextColor(白)` + `SetBkMode(TRANSPARENT)` 画文字 
   抗锯齿边缘的像素会变成**不同程度的灰**；
3. 读取像素，**取 B/G/R 最大值作为覆盖率**（因为黑底白字，亮度就是覆盖率）；
4. 在 Rust 里做标准 over 合成：`na = cov + a0*(1-cov)`，`ncolor = T*cov + bg*(1-cov)`。

这样**文字边缘也是抗锯齿的**。

### 6.10 截图去重（`linker.py`）

- 计算每张图的 **pHash**（感知哈希）；
- 汉明距离  `threshold`（默认 6）视为同一画面，丢弃后来的；
- 保留数量上限 `limit`；
- **缺 imagehash/Pillow 时退化为均匀抽样**，保证永不失败。

### 6.11 颜色字节序（容易搞错）

| 场景 | 编码 | 说明 |
|---|---|---|
| 配置里写给用户看 | `#RRGGBB` | 人类可读 |
| Win32 `COLORREF` | `0x00BBGGRR` | BGR 序 |
| 注册表 `AccentColor` | `0xAABBGGRR` | Alpha 在高位，**低 24 位即 COLORREF** |
| 亚克力 `GradientColor` | `AABBGGRR` | Alpha 在高位 |

`rgb()` 负责打包，`resolve_colors()` 负责解析，`read_system_accent_color()` 负责读注册表。

**文字可读性**：`readable_text_color()` 用相对亮度 `0.2126R + 0.7152G + 0.0722B`，
> 140 用深色字，否则用白字。

---

## 7. 使用指南（CLI 全集）

### 7.1 全局参数

```
vca [--profile <名>] [--config-dir <路径>] [--data-dir <路径>] <子命令>
```

### 7.2 命令一览

| 命令 | 作用 | 状态 |
|---|---|---|
| `vca` | 显示横幅与目录 |  |
| `vca doctor` | 环境自检（ffmpeg/python/soffice） |  |
| `vca config path` | 打印配置与数据目录 |  |
| `vca overlay demo [秒数]` | 显示悬浮窗（默认 8 秒） |  |
| `vca overlay plan [课表]` | 打印今日悬浮窗时序 |  |
| `vca run [课表] [--dry-run]` | **启动守护进程** |  |
| `vca clean [--dry-run]` | 清理到期录像 |  |
| `vca import classisland <json> [--timetable 名]` | 从 ClassIsland 导入 |  |
| `vca import csv <csv>` | 从 CSV 导入课程表 |  |
| `vca import template csv\|timetable` | 输出模板 |  |
| `vca plugin list` | 列出已加载插件 |  |
| `vca plugin info <id>` | 插件详情（含风险提示） |  |
| `vca debug panic` | **验证崩溃静默退出** |  |
| `vca debug crash-info` | 崩溃日志路径与次数 |  |
| `vca debug paths` | 打印全部目录 |  |
| `vca timetable` / `schedule` / `record` / `task` / `market` / `log` / `profile` | 预留占位 |  |

### 7.3 典型工作流

**首次配置（已有 ClassIsland）**：

```bat
scripts\dev.cmd import classisland "<ClassIsland>\data\Profiles\Default.json"
:: 编辑 config\schedule\current.yaml，把要录的课改成 record: true
scripts\dev.cmd overlay plan config\schedule\current.yaml
scripts\dev.cmd run --dry-run
scripts\dev.cmd run
```

**首次配置（无 ClassIsland）**：

```bat
scripts\dev.cmd import template timetable > config\timetable\current.yaml
:: 编辑时间表
scripts\dev.cmd import template csv > D:\课表.csv
:: 用 Excel 填写
scripts\dev.cmd import csv D:\课表.csv
```

**设置密钥（环境变量，绝不入库）**：

```powershell
[Environment]::SetEnvironmentVariable("VCA_LLM_API_KEY",    "<模型Key>",        "User")
[Environment]::SetEnvironmentVariable("VCA_PUSH_ENDPOINT",  "<企业微信Webhook>", "User")
[Environment]::SetEnvironmentVariable("VCA_PUSH_TOKEN",     "<token>",          "User")
[Environment]::SetEnvironmentVariable("VCA_PYTHON_DIR",     "D:\VibeClassAgent\python", "User")
```

### 7.4 环境变量总表

| 变量 | 用途 | 必需 |
|---|---|---|
| `VCA_LLM_API_KEY` | 模型 API Key | 用提取功能时必需 |
| `VCA_PUSH_ENDPOINT` | 推送地址 | 用推送时必需 |
| `VCA_PUSH_TOKEN` | 推送令牌 | 视渠道 |
| `VCA_PROFILE` | 运行身份 | 否（默认 default） |
| `VCA_PYTHON_DIR` | Python worker 目录 | 否（默认 `python`） |
| `VCA_PYTHON` | Python 可执行体 | 否（默认 `python`） |
| `RUST_LOG` | 日志级别 | 否（默认 info） |

---

## 8. 维护指南

### 8.1 改代码前的三条纪律

| 纪律 | 原因 |
|---|---|
| `vca-core` / `vca-ipc` / `vca-engine` 保持 `#![forbid(unsafe_code)]` | 纯逻辑可完整单测 |
| **只有 `vca-platform` 可以用 unsafe**，且每处必须写 `// SAFETY:` 说明为什么安全 | 集中审计，FFI 全在一处 |
| 依赖方向只能向下，不许反向 | 防止循环依赖 |

### 8.2 提交前质量门禁

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

当前基线：**88 测试通过 / clippy 零告警 / fmt 无差异**。

VSCode 里可直接跑任务 `6. quality gate (fmt+clippy+test)`。

### 8.3 常见修改场景

| 想改什么 | 去哪个文件 |
|---|---|
| 产品名 / 数据目录名 | `crates/vca-core/src/lib.rs` 的 `PRODUCT_NAME` / `DEFAULT_DATA_DIR_NAME` |
| 悬浮窗文字/字体/位置/时序/玻璃 | `config/settings.example.yaml` 的 `overlay:` 段；默认值在 `crates/vca-core/src/model.rs` |
| 悬浮窗渲染 | `crates/vca-platform/src/overlay.rs` 的 `render_layered()` |
| 下课/上课判定 | `crates/vca-core/src/schedule.rs` 的 `overlay_windows()` |
| 连堂合并规则 | 同上的 `merge_lessons()` |
| 72 小时保留 | `crates/vca-core/src/cleanup.rs` |
| 崩溃行为与退出码 | `crates/vca-platform/src/crash.rs` |
| ffmpeg 参数 | `crates/vca-platform/src/capture.rs` 的 `build_args()` |
| 处理窗口推导 | `crates/vca-core/src/windows.rs` |
| 提示词 | `crates/vca-platform/src/llm.rs` 的 `SYSTEM_PROMPT` |
| 推送渠道 | `crates/vca-platform/src/push.rs`（加一个 `Pusher` 实现 + 注册到 `make_pusher`） |
| 文档版式 | `python/vca_worker/docgen.py` 的 `build_docx()` |
| 新增 CLI 命令 | `crates/vca-cli/src/main.rs`（enum）+ `cmd.rs`（实现） |
| 新增插件 | 见《Rust 上手文档》与 `plugins/README.md` |

### 8.4 VSCode 配置（`.vscode/`）

| 文件 | 内容 |
|---|---|
| `extensions.json` | 9 个推荐扩展（rust-analyzer、CodeLLDB、Python、YAML、TOML） |
| `settings.json` | 编码/换行/格式、rust-analyzer（用 clippy 检查）、Python、文件嵌套 |
| `tasks.json` | 18 个任务：构建/检查/测试/clippy/fmt、悬浮窗演示、守护进程、插件、清理、Python 自检、**一键质量门禁** |
| `launch.json` | 6 个调试配置（banner/doctor/overlay demo/overlay plan/run daemon/单元测试） |

**所有 task 与 launch 都已注入本地工具链环境变量**（`CARGO_HOME`/`RUSTUP_HOME`/`PATH`）。

**常用快捷键**：

| 键 | 作用 |
|---|---|
| `Ctrl+Shift+B` | 构建 |
| `F5` | 调试运行 |
| `Ctrl+Shift+P`  `Tasks: Run Task` | 任务面板 |
| `Ctrl+Shift+P`  `Rust Analyzer: Restart server` | 语言服务异常时重启 |

> 若 rust-analyzer 报「找不到 cargo」，说明它没读到本地工具链环境变量。
> 建议把 `CARGO_HOME`/`RUSTUP_HOME` 写进系统环境变量（一劳永逸）。

### 8.5 排障顺序

1. `logs/crash.log`  崩溃先看这里（有 panic 位置）
2. `vca debug crash-info`  崩溃次数
3. `vca doctor`  依赖是否齐全
4. `vca plugin list`  插件是否加载
5. `vca overlay plan`  时序是否符合预期
6. `vca clean --dry-run`  清理是否异常
7. `RUST_LOG=debug vca run --dry-run`  详细日志

---

## 9. 测试与质量

### 9.1 测试分布（共 88 个）

| crate | 数量 | 覆盖内容 |
|---|---|---|
| `vca-core` | 35 | 日期算法、排课、单双周、连堂、悬浮窗时序、清理到期、状态机、导入（ClassIsland + CSV）、处理窗口、作业持久化 |
| `vca-platform` | 36 | HTTP URL 解析与 multipart、推送渠道选择与响应解析、LLM 配置与分段与 JSON 抠取、ffmpeg 参数与降档、截图 BMP 头、颜色打包、崩溃码、窗口样式 |
| `vca-engine` | 8 | 流水线降级、截图关联限流去重、清理到期判定、样式映射、默认配置 |
| `vca-plugin-host` | 9 | JSON-RPC 消息、插件发现、市场安装/覆盖/卸载/版本校验 |

### 9.2 测试策略

| 类型 | 做法 |
|---|---|
| **纯逻辑** | 全部用 `#[cfg(test)]` 单测覆盖，不依赖真实环境 |
| **平台相关** | 只测「不依赖真实桌面/网络」的部分（参数拼装、解析、颜色计算）；真实渲染与网络调用**不写自动化测试** |
| **人工验证** | `overlay demo`（看窗口）、`doctor`（看依赖）、`debug panic`（看静默退出）、`import classisland`（看报告） |

### 9.3 已通过的人工验证

| 项 | 验证方式 | 结果 |
|---|---|---|
| 悬浮窗创建/置顶/定位/文字/字体 | Win32 `EnumWindows` 枚举 | 类名 `VcaOverlayWnd`、标题 `Agent Overrr :)`、可见、矩形 `1280,16-1520,88` |
| 悬浮窗圆角抗锯齿与拖动 | 用户目视确认 |  通过 |
| 崩溃静默退出 | 子进程实测 | 无输出、退出码 70、`crash.log` 已写 |
| ClassIsland 导入 | 本机真实数据 | 7 份课表 / 98 条目 / 6 科目待补全 |
| 插件加载 | `plugin list` | 9 个插件，第三方渠道有风险告警 |
| 插件市场 | 单元测试 | 安装/重复拒绝/force 覆盖/卸载全通过 |

---

## 10. 已知限制与待办

### 10.1 为什么不用 chrono / uuid

`uuid` 会经 `getrandom` 触发 **raw-dylib** 链接，需要 `dlltool` 生成导入库；
`chrono` 的时区支持引入类似依赖。在 GNU 工具链（本机无 MSVC）下这步会失败。

**替代方案**：自研 `vca-core/src/time.rs`（civil 日期算法，零依赖），
作业 id 用「日期 + 开始时间 + 课程 + profile」拼字符串（`job_id_for`）。
装上 MSVC C++ 工作负载后可随时加回来。

### 10.2 尚未实现的功能

| 项 | 现状 | 影响 |
|---|---|---|
| `weekend_template.inherit_from` | 字段已建模，**自动套用逻辑未实现** | 沿用历史周末课表需手动复制存档内容 |
| 单双周与学校周次对齐 | 按 ISO 周序号奇偶判定 | 若学校单双周与自然周不同步，需用 `overrides` 校正 |
| `cdylib` / `wasm` 插件模式 | 只实现了 `process` 模式 | 暂无影响（process 是推荐默认） |
| 在线插件市场 | v1 明确不做 | 需手动从本地目录安装 |
| 托盘图标 | 配置项已就位，**图标本身未实现** | 目前无托盘图标（不影响静默性） |
| `timetable` / `schedule` / `record` / `task` 子命令 | 占位 | 用 `run` 与 `import` 已可完成全流程 |
| 插件清单的 checksum 校验 | 未强制 | 安全性依赖来源可信 |

### 10.3 环境依赖（缺失不影响骨架运行）

| 依赖 | 缺失时的影响 |
|---|---|
| ffmpeg | **无法录制**（守护进程会反复重试并记录警告） |
| Python + faster-whisper | 无法转写  流水线降级为「无转写文档」 |
| python-docx | 无法生成文档  作业停在 `Linked` 并标 `Failed` |
| LibreOffice | 只出 Word，不出 PDF（**不算失败**） |
| ImageHash / Pillow | 截图去重退化为均匀抽样 |

---

## 11. 安全规范

1. **凭据零入库**：所有 token / webhook / password 一律走**环境变量**，
   严禁写入代码、`manifest.toml`、`settings.yaml` 或任何入库文件；
2. `.gitignore` 已覆盖 `.env`、`*.pem`、`*.key`、`*credentials*`、`*secrets*`、`*_rsa`，
   以及 `.toolchain/`（含本地工具链与代理配置）、`_tmp/`、`data/`、`logs/`；
3. 提交前检查 `git config` 是否存在 URL 内嵌 token；
4. **录屏与音频绝不上云**；只有**转写后的文本**会发送给用户自己填的 LLM；
5. 第三方协议渠道**必须**填 `risk_note`，启用时展示风险确认；
6. 日志脱敏：密钥只打印前后各 4 位；
7. **数据零预置**：程序出厂不含任何真实学校名、课程名、教师名、作息时间；
   示例文件全部使用「课程一 / 教师甲 / 教室一」等虚构占位。

---

## 12. 附录：快速索引

### 12.1 「我要 X」「改哪里」

| 需求 | 文件 | 关键函数/常量 |
|---|---|---|
| 改产品名 | `crates/vca-core/src/lib.rs` | `PRODUCT_NAME` |
| 改数据目录名 | 同上 | `DEFAULT_DATA_DIR_NAME` |
| 改悬浮窗文字 | `config/settings.example.yaml` + `model.rs` | `OverlaySettings::default()` |
| 改悬浮窗时序 | 同上 | `show_delay_minutes` / `hide_before_minutes` / `fallback_minutes` |
| 改玻璃透明度 | 同上 | `glass_alpha` |
| 关闭系统主机色 | 同上 | `use_system_accent: false` |
| 改录制画质 | `config/settings.example.yaml` | `record.fps` / `height` / `crf` |
| 改转写模型 | 同上 | `transcriber.model` |
| 改保留时长 | 同上 | `cleanup.retention_hours` |
| 改提示词 | `crates/vca-platform/src/llm.rs` | `SYSTEM_PROMPT` |
| 加推送渠道 | `crates/vca-platform/src/push.rs` | 实现 `Pusher` + 注册到 `make_pusher` |
| 改文档版式 | `python/vca_worker/docgen.py` | `build_docx()` |
| 改处理窗口规则 | `crates/vca-core/src/windows.rs` | `WindowSpec` |
| 改连堂规则 | `crates/vca-core/src/schedule.rs` | `merge_lessons()` |

### 12.2 目录速查

| 路径 | 内容 |
|---|---|
| `config/timetable/current.yaml` | **当前生效的时间表** |
| `config/schedule/current.yaml` | **当前生效的课程表** |
| `config/*/archive/` | 版本化存档（永不覆盖） |
| `data/profiles/<profile>/raw/` | 原始录像 |
| `data/profiles/<profile>/docs/` | 生成的 Word/PDF |
| `data/profiles/<profile>/jobs/` | 作业状态与中间产物 |
| `data/profiles/<profile>/logs/crash.log` | 崩溃日志 |

### 12.3 相关文档

| 文档 | 位置 | 适合谁 |
|---|---|---|
| 项目策划书 | `PlanningDocs/项目策划书.md` | 了解需求与设计依据 |
| **本文档** | `PlanningDocs/VibeClassAgent_项目说明.md` | 维护者、接手人 |
| 如何复刻 | `PlanningDocs/如何复刻.md` | 换机器重建 |
| Rust 上手文档 | `PlanningDocs/Rust上手文档.md` | 有 C++/Python 基础者学 Rust |
| 课程表与时间表编辑文档 | `PlanningDocs/课程表与时间表编辑文档.md` | 教师 / 教务 / 运维 |

---

*本文档随代码演进维护。改动核心逻辑时，请同步更新对应章节与 [10](#10-已知限制与待办) 的实现状态表。*


---

## 附录 E：release 构建与体积优化（重要修复）

### E.1 症状

`cargo build`（debug）正常，但 `cargo build --release` 报：

```
error: error calling dlltool 'dlltool.exe': program not found
error: could not compile `windows-sys` (lib)
```

把 Rust 自带的 `dlltool.exe` 加进 PATH 后，变成：

```
error: dlltool could not create import library with ... -d kernel32.dll_imports.def
```

### E.2 根因

`windows-sys` 用了 **raw-dylib** 链接方式，需要 `dlltool` 在链接期生成导入库；
而本机的 GNU 工具链下 `dlltool` 会失败。

**`windows-sys` 从哪来？** 用 `cargo tree -i windows-sys` 查：

```
windows-sys v0.61.2
 anstyle-query    anstream  clap_builder  clap  vca-cli
 anstyle-wincon   anstream （同上）
 nu-ansi-term     tracing-subscriber  vca-cli
```

**结论：全部只来自「终端彩色输出」**。

### E.3 修复

一个后台守护进程**根本不需要彩色终端**。关掉这两个特性即可从依赖树里彻底移除 `windows-sys`：

```toml
# Cargo.toml
clap = { version = "4", default-features = false,
         features = ["std", "derive", "help", "usage", "error-context", "suggestions"] }

tracing-subscriber = { version = "0.3", default-features = false,
                       features = ["std", "fmt", "env-filter"] }
```

同时移除了 `panic = "abort"`（它会破坏 `crash::guard` 的 `catch_unwind`）。

### E.4 效果

| 项 | 修复前 | 修复后 |
|---|---|---|
| `cargo build --release` |  dlltool 失败 |  成功 |
| 依赖树中的 windows-sys | 有 | **无** |
| `vca.exe` 体积 |  | **1,921 KB（约 1.9 MB）** |
| 崩溃静默退出 | 正常 | 正常（`catch_unwind` 也可用） |

**体积为什么这么小**：`opt-level = "z"` + `lto = "thin"` + `codegen-units = 1` + `strip = true`。
这也符合「极低系统占用」的项目目标。

### E.5 给复刻者的提示

如果在别的机器上 `cargo build --release` 也报 dlltool 错误：
**优先检查是不是有依赖引入了终端配色**（`anstream` / `nu-ansi-term` / `windows-sys`）。
把对应的 `color` / `ansi` 特性关掉，比去修 dlltool 简单得多，而且顺带减小体积。


---

## 附录 F：代码审查记录（v0.2 定稿前）

对全项目做了一次系统性自查，方法：自动化扫描 + 人工逐点核实。
共发现 **9 个问题**，全部修复。

### F.1 扫描方法

| 手段 | 目的 |
|---|---|
| 正则扫描生产代码（剔除 `#[cfg(test)]` 段） | 找 `unwrap` / `expect` / `panic!` / `todo!` / 裸索引 / 危险 `as` |
| 跨文件引用统计 | 找「已实现但从未被调用」的死代码 |
| 依赖树分析（`cargo tree -i`） | 找不必要的依赖引入 |
| 逐函数人工核实 | 区分「真缺陷」与「正常的公开 API」 |

### F.2 发现并修复的问题

| # | 严重度 | 问题 | 影响 | 修复 |
|---|---|---|---|---|
| 1 | **高** | `probe::sample_load()` 是空实现，恒返回 0 | **自适应降档完全失效**：无论机器多卡，永远判定为「低负载」，一直用最高画质 | 用 `GetSystemTimes` + `GlobalMemoryStatusEx` + `GetDiskFreeSpaceExW` 实现真实采样；新增 5 个测试 |
| 2 | **高** | 没有 Ctrl+C 处理 | Ctrl+C 立即杀进程，**析构函数不执行**  `CaptureSession::drop` 跑空  **ffmpeg 变孤儿进程 + 录像文件损坏** | 新增 `shutdown` 模块（`SetConsoleCtrlHandler`），收到信号只置标志，主循环走正常退出路径并停掉 ffmpeg |
| 3 | **高** | 守护进程主循环里用了 `?` | 一次磁盘抖动或权限错误就**让整个守护进程退出**，当天后续课程全部不录 | 改为「记录警告 + 下一轮重试」，并校验循环内已无致命 `?` |
| 4 | **中** | `JobState::transition_to()` 从未被调用 | 状态机的合法性校验形同虚设，全靠调用方自觉 | 流水线改用 `advance()` 统一推进，非法迁移会被状态机拒绝 |
| 5 | **中** | `session.rs` 是死代码 | 「服务模式无法采集桌面」这个关键防护**从未生效**，用户误注册成服务只会得到黑屏且无从排查 | 实现真实检测（`ProcessIdToSessionId`），守护进程启动时打印会话类型，Session 0 给出明确告警 |
| 6 | **中** | 插件市场已实现但 CLI 未暴露 | 「本地插件市场」功能**用户够不着** | 新增 `vca plugin install <目录>` / `remove <id>`，安装时展示权限与风险 |
| 7 | **中** | `linker.py` 的 pHash 去重从未被调用 | 文档会塞进大量重复截图，**体积膨胀且无信息量** | 流水线接入 Worker 的 `dedupe`，Worker 不可用时降级为均匀抽样 |
| 8 | **低** | 20 处文档注释声称「未实现」，实际已实现 | **误导后续维护者** | 逐条订正为准确描述 |
| 9 | **低** | 插件目录用相对路径兜底 | 部署时插件会装到意料之外的位置 | 改为多候选探测：可执行体同级  配置同级  工作目录 |

### F.3 额外发现（在修复 2 的过程中）

`Cargo.toml` 里原本有 `panic = "abort"`，它会让 `catch_unwind` 失效
（`crash::guard` 就永远捕获不到）。已移除。

### F.4 复核确认无问题的项

| 检查项 | 结论 |
|---|---|
| 生产代码中的 `unwrap` / `expect` | **0 处**（唯一 `panic!` 在 `debug panic` 诊断命令里，是有意为之） |
| `todo!` / `unimplemented!` | **0 处** |
| 裸切片索引 | 4 处，**全部有长度前置检查**（`parts.len() < 2` 提前 return；`chunks[0]` 由 `chunks.len() == 1` 保证） |
| `as` 截断风险 | 逐处核实为**拓宽转换**（`u8 -> u32`）或**范围已知**（颜色分量、天数、节次），无误截断 |
| 测试覆盖率 | 99 个测试，覆盖全部纯逻辑模块 |
| 依赖树 | 无冗余依赖（`serde_yaml_engine` 已清理） |
| 凭据安全 | 未跟踪文件中无凭据；`.gitignore` 覆盖全部敏感模式；`git config` 无内嵌 token；`.toolchain/` 未入库 |
| 资源泄漏 | `CaptureSession` / `WorkerClient` / `PluginProcess` 均实现 `Drop` 兜底 |

### F.5 最终基线

```
cargo fmt --check           PASS
cargo clippy --all-targets  PASS（零告警）
cargo test                  PASS（99 个）
cargo build --release       PASS
```

发行版冒烟 10 项全通过；可执行文件 **1,956 KB**。


---

## 附录 G：v0.2 完成的功能（原「已知限制」逐项闭环）

### G.1 外部依赖：从「要装两个大件」变成「开箱即用」

| 原状态 | 现方案 | 收益 |
|---|---|---|
| 录制依赖外部 `ffmpeg.exe`（约 106 MB，本机下载源只有 4 KB/s） | **改用 PyAV**wheel 里打包了完整 FFmpeg 库 | 免装、免下载，部署体积反而更小 |
| PDF 依赖 LibreOffice（约 350 MB 安装包） | **改用 fpdf2 直接生成 PDF** | 省掉 350 MB，且不用拉起重量级进程 |
| Python 依赖要联网 `pip install` | **wheel 解压到 `python/vendor/`**，`__init__.py` 自动挂载 | 无网、无管理员权限也能跑 |

**实测证据**（发行版目录内）：

```
$ python -m vca_worker --record --out probe.mp4 --max-seconds 5
{"video":"probe.mp4","seconds":5,"frames":42,"audio_used":false,
 "notes":["未找到可用音频输入设备，本次为纯视频录制"]}
 产出 1,378 KB 的 mp4，可正常解码（h264 / 1152x720 / 8fps）
```

> `vendor/` 已清理无用包（scipy 102 MB、imagehash、PyWavelets），
> 最终 254 MB / 2510 个文件。

### G.2 托盘图标（原「配置项就位、图标未实现」）

`vca-platform/src/tray.rs`：

- 图标由 **GDI 现场绘制**（1616 圆角方块 + 中心白点，抗锯齿），**不依赖 .ico 文件**；
- 右键菜单：`立即停止录制 / 打开日志 / 打开数据目录 / 退出`；
- **只收消息的窗口**（`HWND_MESSAGE`）+ `Shell_NotifyIconW`；
- 全程不含 `NIF_INFO`，**不弹任何气泡**，符合「无打扰」要求。

**踩坑记录**：`NOTIFYICONDATAW` 的字段必须与 Win32 **逐字节一致**
我一开始多加了一个 `szState` 字段，结构体变成 1232 字节（应为 976），
`Shell_NotifyIconW` 会因 `cbSize` 不符静默失败。现已有单测锁死 976。

### G.3 周末课表自动沿用（原「字段已建模、逻辑未实现」）

`ScheduleFile::resolve_weekend()`：

```
weekend_template:
  source: inherit
  inherit_from: "2026-38"     # 指定历史周次
```

守护进程启动时自动去 `config/schedule/archive/` 找 `schedule_2026-38_*.yaml`，
取**版本号最大**的那个，读回它的周末条目。找不到时回退本地 `entries` 并告警。

### G.4 单双周与学校口径对齐（原「按自然周序号奇偶」）

新增 `TermConfig`：

```yaml
term:
  first_monday: "2026-09-01"   # 本学期第 1 周的周一
  first_week_parity: odd       # 第 1 周是单周还是双周
```

`is_odd_week(date)` 按「距第 1 周的周数」计算，**与学校口径一致**；
未配置时退回自然周序号并**在启动时明确告警**，不猜测。

### G.5 插件真正被调度（原「能装能列，但没被调用」）

新增 `PluginRouting`：

```yaml
plugins:
  push: "pusher.wecom"     # 推送交给该插件；留空用内置
  transcribe: ""           # 预留
  docgen: ""               # 预留
```

`Pipeline::push_via_plugin()` 会：
1. 从 `<配置根目录同级>/plugins/` 扫描插件；
2. `PluginProcess::start()` 拉起子进程；
3. `init`  `run`（传文档路径、摘要、目标） 读取 `{success, messageId}`；
4. `shutdown()` 收尾。

测试覆盖：插件发现、JSON-RPC 往返、超时、崩溃隔离。

### G.6 其他修复（本轮代码审查发现）

| # | 问题 | 影响 | 修复 |
|---|---|---|---|
| 1 | 录制视频**时间戳没设置**（`frame.pts = None`） | 时长显示 0.12 秒、帧率 392  文件能解码但时间轴全错 | 显式设 `time_base` + 用帧序号作 PTS，实测 6.25 秒 / 8fps 正确 |
| 2 | 无 Ctrl+C 处理 | 析构不执行  **ffmpeg 孤儿进程 + 录像损坏** | 新增 `shutdown` 模块，信号只置标志，主循环走正常退出路径 |
| 3 | `probe::sample_load()` 空实现 | 自适应降档**完全失效** | 用 `GetSystemTimes` 等实现真实采样 |
| 4 | 守护进程循环内用 `?` | 一次磁盘抖动就**让守护进程退出** | 改为记录警告 + 下一轮重试 |
| 5 | `JobState::transition_to()` 从未被调用 | 状态机校验形同虚设 | 流水线改用 `advance()` 受控推进 |
| 6 | `session.rs` 是死代码 | 「服务模式抓不到屏」防护**从未生效** | 实现 `ProcessIdToSessionId` 检测并接入启动自检 |
| 7 | 插件市场 CLI 未暴露 | 功能用户够不着 | 新增 `plugin install / remove` |
| 8 | pHash 去重从未被调用 | 文档塞满重复截图 | 接入 Worker（并**改用 dHash**，省掉 102 MB 的 scipy） |
| 9 | 插件目录用相对路径兜底 | 部署时装到意料之外的位置 | 三级候选：exe 同级  配置同级  工作目录 |

### G.7 当前仍需注意

| 项 | 说明 |
|---|---|
| **音频采集** | 本机无音频设备，实测为纯视频录制。真实一体机上需要在「声音设置  录制」启用「立体声混音」，或装虚拟声卡（VB-CABLE）。自检会明确提示。 |
| 转写模型 | faster-whisper 首次运行会下载模型（tiny 约 75 MB）。离线环境需预先放入缓存目录。 |
| `plugins.transcribe` / `plugins.docgen` | 配置项已就位，但只有 `push` 真正接入了调度（另两个仍走内置实现）。 |

### G.8 最终基线

```
cargo fmt --check          PASS
cargo clippy --all-targets PASS（零告警）
cargo test                 PASS（109 个）
cargo build --release      PASS（vca.exe 2,018 KB）

发行版 D:\VibeClassAgent\dist\VibeClassAgent\
  256 MB / 2,486 个文件
  冒烟 8 项全通过（含 PyAV 自包含录制产出 mp4）
```


---

## 附录 H：v0.2.1 修复记录（「程序打不开」与运行环境打包）

### H.1 用户反馈的三个问题

| 反馈 | 根因 | 修复 |
|---|---|---|
| **程序无法打开** | `vca.exe` 是**控制台程序**，双击时打印横幅后立即退出，窗口一闪而过 | 双击（控制台里只有自己）时改走**中文交互菜单**，不再一闪而过 |
| **中文乱码** | 简中 Windows 控制台默认代码页 936(GBK)，程序输出的是 UTF-8 | 程序启动时**主动调 `SetConsoleOutputCP(65001)`**，不依赖 `.cmd` 里的 `chcp` |
| **有麦克风但没录到音** | 音频设备名是**硬编码猜测**的，各机器完全不同 | 用 `waveInGetDevCapsW` **枚举真实设备** + 逐个试开 + 结果缓存 |

### H.2 「程序无法打开」的完整解释

`vca.exe` 无参数运行时打印横幅就退出从命令行看是正常行为，
但从资源管理器双击时，控制台窗口会瞬间关闭，用户看不到任何东西，自然会认为「打不开」。

**检测原理**：双击时 Windows 为进程新建一个独立控制台，
`GetConsoleProcessList` 只会返回它自己（1 个）；
而从已有命令行运行时，控制台里至少有 shell 和它自己（2 个）。

```rust
// crates/vca-platform/src/session.rs
pub fn launched_by_double_click() -> bool {
    let n = unsafe { GetConsoleProcessList(buf.as_mut_ptr(), 4) };
    n <= 1     // 0 = 无控制台；1 = 独占控制台
}
```

双击后看到的是：

```
================================================================
  VibeClassAgent  静默课堂录制与课后总结系统
================================================================

  1  环境自检（第一次使用请先跑这个）
  2  显示悬浮窗（看看效果，8 秒后消失）
  3  录制测试（录 10 秒，验证录屏录音是否正常）
  4  从 ClassIsland 导入课表
  5  预览今天的悬浮窗时序
  6  试运行守护进程（不真正录制 / 不真正推送）
  7  正式运行守护进程
  8  预览录像清理（不会真的删除）
  9  查看文档目录
  0  退出

请输入序号后回车：
```

带参数运行时走原来的命令行路径，两者互不干扰。

### H.3 音频设备探测（重写）

**旧做法**：硬编码一串候选名（`virtual-audio-capturer`、`Stereo Mix`、`Microphone`），
逐个试。**在真实笔记本上一个都匹配不上**实际设备名是
`阵列麦克风 (AMD Audio Device)` 这种带厂商与型号的。

**新做法**（`python/vca_worker/recorder.py`）：

```
1. waveInGetDevCapsW 枚举系统里真实存在的输入设备
2. 按优先级排序：内置麦克风 > 普通设备 > 虚拟声卡
   （避免默认录到「UU远程虚拟音频设备」这种远端音频）
3. 逐个用 dshow 试开，第一个成功的就用它
4. 结果写入 .audio_device 缓存，下次直接复用
```

**关键坑**：dshow 遇到打不开的设备时**既不报错也不返回**（实测卡死十几秒），
所以试开必须放在**守护线程里做超时控制**。

**本机实测**（3 个输入设备）：
```
[OK  ] 阵列麦克风 (AMD Audio Device)           选中
[FAIL] 耳机式麦克风 (HUAWEI Sound X-10052)    （未连接）
[FAIL] 麦克风阵列 (UU远程虚拟音频设备)         （虚拟设备）
```

### H.4 录音架构重写（顺带修掉一个隐藏 bug）

**症状**：加上音频后，6 秒录制只得到 13 帧（应为 48 帧），且时长算成 **77738 秒（21 小时）**。

**根因**：
1. 在视频循环里同步等音频帧，dshow 按实时速率交付  拖慢整个循环；
2. 两个设备的时间戳基准不同（音频是 1/10000000，视频是 1/fps），
   直接混用让时长算成了系统开机时长。

**修复**：屏幕与音频**各跑一个读取线程**写入队列，主线程按序取出并**统一打戳**
（视频用帧序号，音频用累计样本数）。

**修复后实测**：
```
时长: 6.75 秒（期望 ~6）
视频: 1152x720 @8fps  54 帧
音频: 44100Hz 2ch     265216 样本 = 6.01 秒
```

### H.5 运行环境打包（真正开箱即用）

之前只打包了第三方库，**Python 解释器本身仍依赖系统安装**。
现在把 **Python 3.14.7 嵌入式运行时**（12 MB 解压后约 25 MB）一并打包：

```
python/
  runtime/             嵌入式 Python（python.exe 104 KB + python314.dll 6.6 MB）
    python314._pth     已改写：只挂载自带的 zip 与同级 vendor
  vca_worker/          我们的代码
  vendor/              全部第三方库
```

`._pth` 内容：

```
python314.zip
.
..\vendor
..
```

**刻意不写 `import site`**  那会把用户的 `%APPDATA%\Python\...\site-packages`
也挂进来，在开发机上可能「碰巧装了某个包」而掩盖缺失依赖，
部署到干净机器上才会暴露。现在路径里只有我们自己的东西。

**解释器选择优先级**（`WorkerClient::resolve_python` / `CaptureSession`）：

1. 环境变量 `VCA_PYTHON`（部署方显式指定）
2. **随包携带的** `python/runtime/python.exe`
3. 系统 PATH 里的 `python`（开发机场景）

### H.6 启动脚本改进

- 每个 `.cmd` 开头加 `chcp 65001 >nul`（输出 UTF-8）；
- 用 `python\runtime\python.exe` 而不是系统 `python`；
- 新增 `2-record-test.cmd`：**10 秒录制冒烟测试**，直接产出 mp4，
  用来一键验证「录屏 + 录音」链路是否正常。

### H.7 最终验证

```
cargo fmt --check          PASS
cargo clippy --all-targets PASS（零告警）
cargo test                 PASS（109 个）
cargo build --release      PASS（vca.exe 2,030 KB）

发行版：288 MB / 2,984 个文件（含 Python 运行时）

实测（发行版内，全部使用内置运行时，不依赖任何系统安装）：
  1-selftest.cmd      依赖全 OK；屏幕采集 OK；音频选中「阵列麦克风」
  2-record-test.cmd   test.mp4  2,487 KB / 9 秒 / 1152x720@8fps / 含音频轨
  双击 vca.exe        显示中文交互菜单（不再一闪而过）
  中文输出            正确（实测「环境自检」等字样显示正常）
```


---

## 附录 I：首次运行体验（自动修复 + 引导向导）

### I.1 问题

之前的假设是「用户会先读文档再配环境」。实际上第一次打开的人只会双击文件，
遇到缺失就卡住。两个具体缺口：

1. **自检只报错、不修复**  告诉用户「缺配置」，但不帮他建；
2. **没有新用户引导**  从「打开程序」到「能录课」中间的步骤全靠文档。

### I.2 新增：`vca doctor --fix`（自动修复）

幂等，可反复执行。只做**不需要询问用户**的事：

| 自动做 | 说明 |
|---|---|
| 建目录骨架 | 配置/数据/存档/作业/日志/文档 共 9 个目录 |
| 生成 `settings.yaml` | 从**编译进二进制**的内置模板生成 |
| 生成 `timetable/current.yaml` | 同上 |
| 生成 `schedule/current.yaml` | 同上 |
| 检查配置内容 | 指出占位符、未勾选录制的课、没有长休息段等问题 |
| 可写性探针 | 往数据目录写一个临时文件再删掉，确认有权限 |

模板用 `include_str!` 编译进 exe，所以**只拷走一个 `vca.exe` 也能生成全套配置**。

实测（空目录）：

```
$ vca doctor --fix
+ 新建目录  ...\cfg
+ 新建目录  ...\cfg\profiles\default
...（共 9 个目录）
+ 生成文件  ...\settings.yaml
+ 生成文件  ...\timetable\current.yaml
+ 生成文件  ...\schedule\current.yaml
新建目录 9 个 / 新建文件 3 个 / 修正 1 项 / 待用户提供 4 项 / 警告 0 条
```

### I.3 新增：`vca setup`（引导向导）

五步，每步都可直接回车跳过：

```
[1/5] 环境检查与自动修复
      + 已生成 ...\settings.yaml
       已补齐 9 个目录 / 3 个文件
      正在检测录屏与麦克风（首次可能要几秒）
      OK 屏幕采集 1920x1200 / 麦克风：阵列麦克风 (AMD Audio Device)

[2/5] 课表
      a) 从 ClassIsland 导入（推荐）
      b) 从 CSV 导入
      c) 稍后自己编辑
      输入 ClassIsland 的 Default.json 路径（回车跳过）：
       导入后还会问「要现在把全部课都设为要录吗？[y/N]」

[3/5] 学期对齐（决定「单周/双周」怎么算）
      本学期第 1 周的周一日期（如 2026-09-01，回车跳过）：
      该周是单周还是双周？[1=单周 / 2=双周]

[4/5] 模型 API（把转写文字提炼成课堂要点）
      base_url（回车跳过）：
      模型名：
       提醒：API Key 走环境变量 setx VCA_LLM_API_KEY

[5/5] 推送渠道
      1) 企业微信机器人  2) 通用 Webhook  3) Server 酱  0) 先不配
```

完成后自动写 `.initialized` 标记，并打印接下来的 5 条建议命令。

**重新引导**：`vca reset-setup` 会清掉标记，下次 `vca setup` 再来一遍。

### I.4 双击体验升级

双击 `vca.exe`（独占一个控制台）时显示中文菜单，现在把引导放在第一位：

```
  1  首次引导向导（**第一次使用请先跑这个**）
  2  环境自检 + 自动修复
  3  显示悬浮窗
  4  录制测试
  5  从 ClassIsland 导入课表
  6  预览今天的悬浮窗时序
  7  试运行守护进程
  8  正式运行守护进程
  9  预览录像清理
  d  打开文档目录
  0  退出
```

### I.5 首次运行检测

`vca run` 启动时会检查 `.initialized` 标记：

- **没有标记**  自动跑一次 `repair`，把「还需要你提供」的项目逐条打进日志，
  然后**标记为已初始化**（避免每次启动都刷屏）；
- 用户随时可以用 `vca setup` 或 `vca reset-setup` 回到引导。

### I.6 顺带修掉的两个真实 bug

| # | 问题 | 影响 | 修复 |
|---|---|---|---|
| 1 | **时间表模板用的是 `type:`，但结构体字段名是 `kind:`** | **示例配置本身解析失败**  照文档填的时间表根本加载不了 | 给字段加 `#[serde(alias = "type")]`，两种写法都支持；同时统一示例为 `type` |
| 2 | 自检把 `ffmpeg` / `soffice` 列为「缺失」 | 现在功能都已内置（PyAV / fpdf2），报缺失**误导用户去装 100MB + 350MB 的东西** | 改为 `[SKIP]` 并注明「不需要」；`python` 也标为可选（发行版自带运行时） |

### I.7 最终验证

```
cargo fmt --check          PASS
cargo clippy --all-targets PASS（零告警）
cargo test                 PASS（123 个，较上轮 +14）
cargo build --release      PASS（vca.exe 2,077 KB）

端到端（模拟全新用户，发行版内）：
  场景 1  doctor --fix        自动建 9 目录 + 生成 3 配置，零手工干预
  场景 2  配置可加载           settings / timetable / schedule 全部就绪
  场景 3  幂等               再跑一次「目录与配置都已就绪，无需修复」
  场景 4  setup 向导          五步走完，实测探到「屏幕采集 1920x1200 / 麦克风：阵列麦克风」
```
