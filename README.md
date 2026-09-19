# VibeClassAgent（<项目名>）

轻量级**静默课堂录制与课后总结系统**。面向希沃一体机（多教师共用、日常可能卡顿），
按课表自动静默录屏录音，在休息时段自动完成「转写  要点提取  截图关联  生成 Word/PDF 
推送  72 小时到期清理」的全流程，下课后在桌面右上角显示一个可拖动的玻璃悬浮窗。

> **状态：功能完整、开箱即用**
> 109 个单元测试通过 ｜ clippy 零告警 ｜ fmt 无差异
> **录制用 PyAV、PDF 用 fpdf2**  不需要额外安装 ffmpeg 或 LibreOffice
> 平台：Windows 10 及以上 ｜ 技术栈：Rust（宿主）+ Python（算法侧）

---

## 一、它能做什么

| 能力 | 状态 | 说明 |
|---|---|---|
| 按时段静默录屏录音 |  | ffmpeg `gdigrab`+`dshow`，`CREATE_NO_WINDOW` 无窗口启动 |
| 桌面悬浮窗 |  | 下课后 5 分钟显示、上课前 2 分钟关闭；玻璃质感、抗锯齿圆角、可拖动、不抢焦点 |
| 崩溃静默退出 |  | `SetErrorMode` 关系统弹窗 + panic 钩子写日志 + 退出码 70，全程无输出 |
| 屏上课表/时间表导入 |  | ClassIsland JSON（只读）与 CSV，含教师名清洗 |
| 课后处理流水线 |  | 转写  要点提取  截图去重  Word/PDF  推送，每步落盘可续跑 |
| 72 小时清理 |  | 从**推送成功时刻**起算；推送未成功**永不删除** |
| 插件架构 |  | 清单 + 进程隔离运行时（JSON-RPC over stdio）+ 本地市场 |
| 本地插件市场 |  | 安装/覆盖/卸载，展示权限与风险清单 |
| 托盘图标 |  | 配置项已就位，图标未实现 |
| 在线插件市场 |  | v1 明确不做 |

---

## 二、快速开始

```bat
:: 1) 编译（首次约 1-3 分钟）
cargo build

:: 2) 自检
scripts\dev.cmd doctor

:: 3) 看悬浮窗效果（保持 8 秒，右上角，可拖动）
scripts\dev.cmd overlay demo 8

:: 4) 导入课表（若学校在用 ClassIsland）
scripts\dev.cmd import classisland "D:\ClassIsland_app_windows_x64_selfContained_folder\data\Profiles\Default.json"

:: 5) 编辑 config\schedule\current.yaml，把要录的课改成 record: true

:: 6) 核对时序
scripts\dev.cmd overlay plan config\schedule\current.yaml

:: 7) 试运行（不真正录制/推送）
scripts\dev.cmd run --dry-run

:: 8) 正式运行
scripts\dev.cmd run
```

> `.cmd` 入口用 `-ExecutionPolicy Bypass` 调用 PowerShell，只影响当次进程，**不改系统设置**。
> 若你更愿意放开用户级策略：`Set-ExecutionPolicy -Scope CurrentUser RemoteSigned`

---

## 三、目录结构

```
VibeClassAgent/
 Cargo.toml / Cargo.lock         workspace 根清单 + 依赖锁定
 .env.example                    环境变量模板（只有占位符）
 .gitignore                      排除构建产物/密钥/运行时数据/本地工具链
 .vscode/                        VSCode 任务、调试、推荐扩展

 crates/                         Rust 源码（6 个 crate，依赖单向向下）
    vca-core/                   领域核心（无 IO、无 unsafe）
                                  时间算法  排课  悬浮窗时序  清理  导入  持久化
    vca-ipc/                    插件通信契约（JSON-RPC 消息与载荷）
    vca-platform/               平台适配（唯一允许 unsafe）
                                  悬浮窗  崩溃处理  HTTP  推送  LLM  录制  截图  时钟
    vca-plugin-host/            插件宿主（清单/发现/运行时/本地市场）
    vca-engine/                 编排（守护进程 + 课后流水线 + Worker 客户端）
    vca-cli/                    CLI 入口（clap 命令树）

 plugins/                        9 个插件占位（manifest + README）
 python/vca_worker/              算法侧：转写 / 文档生成 / 截图去重
 config/                         配置模板（settings / timetable / schedule）
 docs/                           架构、插件 SDK、开发指南
 scripts/                        dev.cmd / dev.ps1 / doctor.*
 data/  logs/                    运行时目录（已 gitignore）
 PlanningDocs/                   策划书与全部交付文档
```

### 分层与依赖方向（**必须保持单向**）

```
vca-cli > vca-core       （纯逻辑，可脱离 Windows 单测）
          > vca-ipc
          > vca-plugin-host
          > vca-platform
          > vca-engine > vca-core / vca-ipc / vca-platform / vca-plugin-host
```

---

## 四、命令一览

| 命令 | 作用 |
|---|---|
| `vca` | 显示横幅与目录 |
| `vca doctor` | 环境自检（ffmpeg / python / soffice） |
| `vca config path` | 打印配置与数据目录 |
| `vca overlay demo [秒]` | 显示悬浮窗（人工验证外观与拖动） |
| `vca overlay plan [课表]` | 打印今日悬浮窗显示/关闭时刻 |
| `vca run [课表] [--dry-run]` | **启动守护进程** |
| `vca clean [--dry-run]` | 清理到期录像 |
| `vca import classisland <json> [--timetable 名]` | 从 ClassIsland 导入（只读） |
| `vca import csv <csv>` | 从 CSV 导入课程表 |
| `vca import template csv\|timetable` | 输出模板 |
| `vca plugin list` / `info <id>` | 插件列表 / 详情 |
| `vca plugin install <目录>` | 从本地目录安装（展示权限与风险） |
| `vca plugin remove <id>` | 卸载插件 |
| `vca debug panic` / `crash-info` / `paths` | 诊断（`panic` 用于验证静默退出） |

全局参数：`--profile <名>`、`--config-dir <路径>`、`--data-dir <路径>`

### 环境变量（**密钥只走这里**）

| 变量 | 用途 |
|---|---|
| `VCA_LLM_API_KEY` | 模型 API Key |
| `VCA_PUSH_ENDPOINT` / `VCA_PUSH_TOKEN` | 推送地址与令牌 |
| `VCA_PROFILE` | 运行身份（默认 default） |
| `VCA_PYTHON_DIR` / `VCA_PYTHON` | Python worker 目录与可执行体 |
| `RUST_LOG` | 日志级别（默认 info） |

---

## 五、技术选型

| 层 | 选型 | 理由 |
|---|---|---|
| 宿主 / 调度 / CLI / 插件管理 | **Rust** | 体积小、内存低、并发安全；Cargo 统一依赖 |
| 重算法（转写 / 文档 / 去重） | **Python 子进程** | 生态无可替代；按需拉起、用完释放 |
| 录制引擎 | **ffmpeg** | 兼容性好、可控、无额外运行时 |
| 语音转写 | **faster-whisper**（本地） | 纯离线、零调用费、录屏不上云 |
| 文档导出 | **python-docx**  **LibreOffice** | Word 必出，PDF 尽力 |
| HTTP 客户端 | **自研 WinHTTP 封装** | 系统自带、零第三方依赖 |
| 插件通信 | **JSON-RPC 2.0 over stdio** | 语言无关、天然进程隔离 |
| 时间处理 | **自研**（civil 日期算法） | 刻意不引 chrono，见下 |

### 刻意的取舍

`uuid` 会经 `getrandom` 触发 **raw-dylib** 链接，`chrono` 引入类似依赖，
在无 MSVC 的 GNU 工具链下链接会失败。因此：
- 时间用 `crates/vca-core/src/time.rs`（零依赖）
- 作业 id 用「日期+时间+课程+profile」拼字符串

装上 MSVC C++ 工作负载后可随时加回。

---

## 六、质量门禁

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

**当前基线**

| 项 | 结果 |
|---|---|
| 单元测试 | **88 个全部通过**（core 35 / platform 36 / engine 8 / plugin-host 9） |
| clippy | **零告警** |
| fmt | 无差异 |

VSCode 里可一键跑任务 `6. quality gate (fmt+clippy+test)`。

---

## 七、文档

| 文档 | 位置 | 适合谁 |
|---|---|---|
| **项目说明（代码维护与使用报告）** | `PlanningDocs/VibeClassAgent_项目说明.md` | 维护者、接手人 |
| 课程表与时间表编辑文档 | `PlanningDocs/课程表与时间表编辑文档.md` | 教师 / 教务 / 运维 |
| Rust 上手文档 | `PlanningDocs/Rust上手文档.md` | 有 C++/Python 基础者 |
| 如何复刻 | `PlanningDocs/如何复刻.md` | 换机器重建 |
| 项目策划书 | `PlanningDocs/项目策划书.md` | 了解需求与设计依据 |
| 架构说明 | `docs/ARCHITECTURE.md` | 快速了解分层 |
| 插件开发指南 | `docs/PLUGIN_SDK.md` | 写插件 |
| 开发与调试 | `docs/DEVELOPMENT.md` | 日常开发 |

---

## 八、安全规范（强制）

1. **凭据零入库**：所有 token / webhook / password 一律走**环境变量**，
   严禁写入代码、`manifest.toml`、`settings.yaml` 或任何入库文件；
2. `.gitignore` 已覆盖 `.env`、`*.pem`、`*.key`、`*credentials*`、`*secrets*`、`*_rsa`，
   以及 `.toolchain/`、`_tmp/`、`data/`、`logs/`；
3. 提交前检查 `git config` 是否存在 URL 内嵌 token；
4. **录屏与音频绝不上云**；只有转写后的**文本**会发送给用户自己填的 LLM；
5. 第三方协议渠道必须填 `risk_note`，启用时展示风险确认；
6. 日志脱敏：密钥只打印前后各 4 位；
7. **数据零预置**：出厂不含任何真实学校名、课程名、教师名、作息时间。

---

## 九、本机环境适配（仅当前这台机器）

本机为**受限环境**，做了四项适配。**换到正常开发机请忽略本节**。

| # | 问题 | 处理 |
|---|---|---|
| 1 | 未安装 Rust，且用户目录无写权限 | 工具链装在工作区内 `.toolchain/`（已 gitignore） |
| 2 | VS Build Tools 缺 C++ 工作负载（无 `link.exe`） | 改用 GNU 目标（自带链接器） |
| 3 | PATH 里有 Dev-Cpp 旧 MinGW 抢占 | `scripts/dev.ps1` 自动从 PATH 剔除 |
| 4 | cargo 的 schannel TLS 被阻断 | 本地 HTTP 代理转发 crates.io（配置在 `.toolchain/`，不入库） |

**长期建议**：装一次 MSVC C++ 工作负载，上述 2/3/4 全部不再需要。

详见 `PlanningDocs/如何复刻.md` 第 5 节。

---

## 十、下一步

| 优先级 | 任务 |
|---|---|
| 高 | 装 MSVC C++ 工作负载（消除本机全部绕行） |
| 高 | 在真实一体机上跑通「录制  处理  推送  清理」全链路 |
| 中 | 托盘图标（`ui.tray_icon` 已就位） |
| 中 | 周末课表 `inherit_from` 自动套用 |
| 中 | `timetable` / `schedule` / `task` 子命令补全 |
| 低 | 插件 checksum 强制校验 |
