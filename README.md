# VibeClassAgent

轻量级**静默课堂录制与课后总结系统**，面向希沃一体机等教学大屏。

按课表在上课时段静默录屏录音（含**系统声音**与麦克风），在午休/放学后自动完成
「语音转写 → 要点提取 → 截图关联 → 生成带图文档 → 推送到微信/QQ → 到期清理」全流程。
全程无提示、无弹窗、无托盘打扰。

> **技术栈**：Rust 单语言，无 Python 运行时。
> **平台**：Windows 10 及以上。
> **测试**：188 个单元测试全绿。

---

## 一、它能做什么

| 能力 | 状态 | 说明 |
|---|---|---|
| 按时段静默录屏录音 | ✅ | ffmpeg 录屏 + **WASAPI 采系统声音与麦克风**，`CREATE_NO_WINDOW` 无窗口启动 |
| 硬件编码自动择优 | ✅ | 实测探测 QSV/NVENC/AMF，不可用自动退 x264 |
| 本地语音转写 | ✅ | whisper.cpp，离线、零调用费；线程数默认只吃一半核心 |
| 云端转写（可选） | ✅ | OpenAI 兼容 `/v1/audio/transcriptions` |
| 要点提取（三模式） | ✅ | 付费 API / **免费额度模型** / 浏览器自动化网页版 |
| 截图关联 | ✅ | dHash 去重 + 按时间戳把讲稿配成图注 |
| 带图文档生成 | ✅ | Markdown（总生成）+ Word（docx-rs）+ PDF（Edge 无头打印） |
| 微信 / QQ 推送 | ✅ | 企业微信、**QQ 官方机器人**、**OneBot（自建 QQ）**、通用 Webhook、Server 酱、个人微信中转 |
| 推送失败重试 | ✅ | 2/4/8s 递增退避；**全部失败则不删除本地录像** |
| 72 小时到期清理 | ✅ | 从**推送成功时刻**起算 |
| 交互式命令行 | ✅ | Claude Code 风格：欢迎框、`> ` 提示符、斜杠命令、⏺/⎿ 结果标记 |
| 插件架构 | ✅ | 清单 + 进程隔离运行时（JSON-RPC over stdio） |

---

## 二、快速开始

### 1. 取运行时依赖（约 200 MB，不进仓库）

```bash
python scripts/fetch-deps.py                # ffmpeg + whisper.cpp + tiny 模型
python scripts/fetch-deps.py --models base  # 想要更准的模型
```

产物落在 `tools/`：

```
tools/ffmpeg/ffmpeg.exe           屏幕采集与音视频合并
tools/whisper/whisper-cli.exe     本地语音转写
tools/whisper/models/ggml-*.bin   转写模型
```

> 换机器时跑一次即可。脚本支持代理（`VCA_PROXY`）、已存在则跳过、多源自动回退。
> 也可以自己准备这些文件放进去，或用环境变量指定路径。

### 2. 编译

```bash
scripts\dev.cmd doctor      # 环境自检
scripts\dev.cmd             # 直接进入交互界面
```

首次编译约 1–3 分钟。

### 3. 用起来

```bash
vca                         # 交互界面（推荐）
vca --help                  # 子命令一览
```

进入交互界面后：

```
> /status        总览：作业、组件就绪情况
> /doctor fix    自检并自动生成配置
> /record-test   录 10 秒冒烟测试（含截图与合并）
> /process       处理待办作业（转写 → 提取 → 文档 → 推送）
> /help          全部命令
```

配置好之后，日常只需要 `vca run` 挂后台（或注册成计划任务），其余全自动。

---

## 三、配置

配置文件在 `<配置目录>/profiles/<profile>/settings.yaml`，`vca` 首次运行会生成模板。

**凭据一律走环境变量，绝不写进配置文件。**

| 环境变量 | 用途 |
|---|---|
| `VCA_LLM_API_KEY` | 要点提取的模型 API Key |
| `VCA_STT_API_KEY` | 云端转写 Key（用本地转写则不需要） |
| `VCA_PUSH_TOKEN` | 推送令牌（企业微信 key / OneBot access_token） |
| `VCA_PUSH_ENDPOINT` | 推送地址（OneBot 基址等） |
| `VCA_QQ_APP_ID` / `VCA_QQ_APP_SECRET` | QQ 官方机器人凭据 |
| `VCA_FFMPEG` / `VCA_WHISPER` / `VCA_WHISPER_MODEL` | 覆盖外部组件路径 |
| `VCA_TOOLS_DIR` | 覆盖 `tools/` 所在目录 |
| `VCA_PROXY` | HTTP 代理（受限网络下用得到） |

### 要点提取的三种模式

```yaml
llm:
  mode: paid-api        # paid-api | free-api | browser-bot
  provider: deepseek    # deepseek / zhipu / dashscope / moonshot /
                        # siliconflow / openai / groq / openrouter / ollama / custom
  model: deepseek-chat
```

| 模式 | 成本 | 稳定性 | 说明 |
|---|---|---|---|
| `paid-api` | 按量计费 | 高 | 官方付费接口 |
| `free-api` | **0** | 高 | 换成有免费额度的模型（如 `glm-4-flash`、`qwen-turbo`）。**这是「免费」里唯一稳的一条**：同一套协议，厂商改政策只需换模型名 |
| `browser-bot` | 0 | 中低 | 驱动系统自带 Edge 去问网页版聊天机器人。**需显式 `risk_ack: true`**，可能违反第三方 ToS，账号有风险 |

内置厂商预设表见 `crates/vca-platform/src/llm.rs` 的 `PROVIDERS`；
地址与模型名**以各厂商当前文档为准**，预设只是起点，随时可覆盖。

### 转写

```yaml
transcriber:
  engine: local         # local | cloud | auto（本地不可用才走云端）
  model: tiny           # tiny | base | small
  threads: 0            # 0 = 自动（核数一半，上限 4）
  max_seconds: 7200     # 超过这个时长不本地跑，避免占死上课用的机器
```

### 推送

```yaml
push:
  provider: onebot      # wecom | qq(官方) | onebot(第三方) | webhook | serverchan | wechat-personal
  target: "123456789"   # QQ 号 / 群号 / openid
  target_type: private  # private | group
  max_retries: 3
```

> ⚠️ OneBot 与个人微信属于**第三方协议实现**（非腾讯官方）。自动化登录存在账号
> 被限制或封禁的风险，程序会在启用时提示，请自行评估并优先使用小号。
> QQ 官方机器人合规但需审核，且 `target` 是 **openid 而非 QQ 号**。

---

## 四、实测数据

在 i5-10400 / 8 GB（**性能最差的测试机**）上：

| 项 | 实测值 |
|---|---|
| 录制产物 | 10.4 秒 720p@8fps → **396 KB**（1152×720 h264 + 48 kHz 立体声 aac，305 kb/s） |
| 换算 | 一节课 45 分钟约 **105 MB** |
| 本地转写 | 10 秒音频 → **1.0–1.4 秒**（whisper.cpp tiny，含 ffmpeg 转码） |
| 音频采集 | 系统回环 48 kHz 2ch + 麦克风 48 kHz 1ch |
| 端到端 | 录制 → 转写 → 截图关联 → 文档 → 推送 → 登记清理，全流程跑通 |

---

## 五、架构

```
crates/
  vca-core/         领域核心（无 IO、无 unsafe）
                      时间算法 · 课表排课 · 作业状态机 · 清理策略 · 导入 · 持久化
  vca-ipc/          插件通信契约（JSON-RPC 消息与载荷）
  vca-platform/     平台适配（唯一允许 unsafe）
                      audio     WASAPI 系统回环 + 麦克风采集
                      capture   录制会话（ffmpeg 视频 + 编码器实测择优 + 收尾合并）
                      stt       语音转写（本地 whisper.cpp / 云端 API）
                      llm       要点提取（三模式 + 厂商预设）
                      browser_bot  CDP 驱动系统 Edge
                      docgen    文档生成（Markdown / Word / PDF）
                      shots     截图去重与时间对齐
                      push      六种推送渠道
                      http      ureq + rustls 封装（代理、连接池）
                      proc      静默子进程与外部工具查找
  vca-plugin-host/  插件宿主（清单 / 发现 / 运行时 / 本地市场）
  vca-engine/       编排（守护进程 daemon + 课后流水线 pipeline）
  vca-cli/          入口（子命令 + 交互式界面 repl）
```

依赖方向单向向下，`core` 与 `ipc` 不反向依赖任何上层 crate。

### 课后流水线的状态机

```
Pending → Recorded → Transcribed → Extracted → Linked → DocReady → Pushed → Cleaned
                                                  任意状态 → Failed →（可重试）
```

**`Pushed` 是 72 小时倒计时的唯一起点**，且每次迁移都由状态机校验，
推送未成功则不进入倒计时、不删除任何文件。

### 中间产物与抗中断

录制期间落盘的是**中间产物**，收尾失败可重跑，不必重录一节课：

```
<job>/
  _work_<名>/video.mkv     视频（matroska：被强杀也通常能播；mp4 会丢 moov 而全损）
  _work_<名>/audio/*.wav   系统声 / 麦克风
  _work_<名>/ffmpeg.log    出问题时的现场
  _stt/                    转写的中间音频
  transcript.txt/json      转写全文 + 带时间戳分段
  summary.json             结构化要点
  shot_refs.json           去重后的截图与图注
```

---

## 六、已知限制与待确认项

- **PDF 生成依赖系统 Edge/Chrome**。原理是 HTML → 浏览器无头打印（中文渲染零配置、
  体积小）。若浏览器不可用，会**保留 HTML** 并在备注里说明，可用浏览器打开后另存为 PDF。
  > 受限会话（远程桌面、强策略企业环境、沙箱）里 Chromium 的命名管道 IPC 会被拒，
  > 表现为 `FATAL:mojo platform_channel: Access is denied`。程序会自动用单进程模式重试一次。
- **浏览器自动化模式（`browser-bot`）未经真实站点验证**。各站点 DOM 与登录策略随时会变，
  默认走差分法（比对发送前后页面正文的变化）而非硬编码选择器，改版时更耐用，
  但仍可能需要用 `llm.browser.selectors` 覆盖。
- **QQ 官方机器人的 `target` 是 openid**，需要从机器人收到的事件里取，不是 QQ 号。
- **转写速度与模型质量随机器差异较大**，上表的数字出自 i5-10400，其他机型需实测。
- **未在真实一体机上课时段长期运行验证过**，建议先跑一周观察磁盘占用与日志。
- `plugins/` 下的内置插件仍是占位骨架（只有清单与说明），插件宿主本身可用。

---

## 七、开发

```bash
scripts\dev.cmd doctor              # 自检
scripts\dev.cmd -Check              # 只做 cargo check
cargo test --workspace              # 188 个测试
cargo clippy --all-targets -- -D warnings
```

本机若是受限环境（无 MSVC、有旧 MinGW 抢占 PATH），`scripts/dev.ps1` 已经处理了
三处适配，细节见该文件里的注释：为 dlltool 准备 as.exe、用 CC/AR 指定 C 编译器
（但**不能**把 MinGW 放进 PATH，否则它的 ld 会被 rustc 当链接器）。

**装上一次 MSVC C++ 工作负载，这些适配就都不需要了。**

---

## 八、安全

1. **凭据零入库**：所有 token / webhook / password 只走环境变量；
   `.gitignore` 覆盖 `.env`、`*.pem`、`*.key`、`*credentials*`、`*secrets*`、`*_rsa`；
2. **录屏与音频绝不上云**。只有「转写成文本之后」的内容才会发送给你自己配置的模型；
3. 第三方协议渠道必须显式确认风险才允许启用；
4. 日志脱敏，密钥只打印前后各 4 位；
5. **数据零预置**：出厂不含任何真实学校名、课程名、教师名、作息时间。
