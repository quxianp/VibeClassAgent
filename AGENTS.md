# AGENTS.md

> 工作区级 AI Agent 指令。**接手这个项目前请先读完本节与 `HANDOFF.md`。**

## 这是什么项目

**VibeClassAgent（VCA）** —— 轻量级静默课堂录制与课后总结系统，运行在 Windows / 希沃一体机上。

按课表在上课时段静默录屏录音（含**系统声音**与麦克风），课后自动完成
「语音转写 → 要点提取 → 截图关联 → 生成带图文档 → 推送到微信/QQ → 到期清理」。

- 技术栈：**纯 Rust**（无 Python 运行时）
- 规模：约 1.2 万行源码，39 个 `.rs`
- 状态：**四大核心功能已实测通过**，256 个单元测试全绿，clippy 零告警（`scripts\gate.cmd`）

## 接续工作前必读

**`HANDOFF.md`** 是完整的交接文档：架构地图、关键决策与理由、踩过的 18 个坑、
已验证/未验证的诚实清单、未完成事项与优先级、推送凭据的取法。

**不要从头读一遍代码再猜上下文** —— 那正是低效的做法，会重复上一任的错误。

## 三条硬规矩

1. **文档里不写没验证过的数字或功能**。上一任 agent 的 README 在同一页里
   写着「109 个测试」和「88 个测试」（实测 123），还把只有空壳的功能写成"已实现" ——
   这是用户给出"太垃圾了"评价的直接原因。**不确定就标注为待确认。**
2. **绝不提交凭据**。token / webhook / password 一律走环境变量或
   `secrets.env`（已被 `.gitignore` 排除）。推送 GitHub 时注意别把 token 打进输出。
3. **改动过三关再提交**：直接跑 `scripts\gate.cmd`（= `cargo fmt --check` →
   `clippy --workspace --all-targets -- -D warnings` → `cargo test --workspace`），
   三关全过才提交。clippy 加了 `-D warnings`，所以「一个告警」也会让 gate 红。
   手工跑也是这三条，但漏掉 clippy 的概率很高——这是加脚本的原因。

## 常用命令

```powershell
cd D:\VibeClassAgent
.\scripts\gate.cmd             # 提交前的三道关（fmt / clippy / test）
.\scripts\gate.cmd -SkipTest   # 只想快速看格式与静态检查
.\scripts\dev.cmd              # 编译并进交互界面（数字菜单 + / 命令）
.\scripts\dev.cmd -Check       # 只做 cargo check
.\scripts\dev.cmd doctor       # 环境自检
.\scripts\dev.cmd debug e2e    # 端到端冒烟（录 15 秒跑完整流程）
.\scripts\dev.cmd plugin list  # 插件列表
python scripts\smoke.py         # 图形界面的端到端冒烟（起服务打真实 HTTP 接口）
```

> `dev.cmd` 与 `gate.cmd` 共用 `scripts\env.ps1` 里的环境准备（本地工具链、
> 剔除旧 MinGW、dlltool 的 as.exe、CC）。改工具链设置只改那一处，
> 否则会出现「开发能跑、gate 说找不到 gcc」这种极难查的漂移。

> **PowerShell 里当前目录的脚本必须加 `.\` 前缀**，否则报
> `The module 'scripts' could not be loaded`。
> 不想 cd 就用 `& 'D:\VibeClassAgent\scripts\dev.cmd' ...`。

## 本机环境的四个坑（已封装进 dev.ps1 / gate.ps1，但直接跑 cargo 时要自己注意）

1. **不要在 workspace 内用 `cargo init` 建探针项目** —— 它会污染根 `Cargo.toml` 的 members
2. **编辑任何 `.ps1` 后必须补 UTF-8 BOM** —— PowerShell 5.1 按 ANSI 读无 BOM 文件，中文注释会破坏语法
3. **8 GB 内存限制**：`cargo test` 用 `-j 1`，否则可能 `Allocation failed`
4. **新增依赖会去连本地 sparse 代理（`127.0.0.1:13579`，见 `.toolchain/cargo/config.toml`）** ——
   代理没在跑时 cargo 只会一遍遍刷 `spurious network error`，看起来像卡死（实测卡过一次 gate）。
   两条路：**把代理起起来**（`python _tmp/regproxy.py`，它监听 13579；
   注意它既要转发 index 也要转发 `/dl/` 的包体下载，少一段就会 404）；
   或者 `--offline` 配合**收窄依赖特性**。
   收窄这条更常用：默认特性常常拖进一串本机缓存里没有的 crate
   （`zip` 就是这么处理的 —— 关掉 default，只留 `deflate-flate2`）。

## 临时文件约定

一次性脚本、探针、抓取结果一律放 `_tmp/`（已 gitignore，会被自动清理）。
**正式代码与文档不要放进 `_tmp/`。**

## 当前待办（详见 `HANDOFF.md` §9）

| 优先级 | 事项 |
|---|---|
| ✅ | **daemon 真实时段验证** —— 已在真实时间轴上跑完整条链路（到点开录 → 收尾 → 处理窗口自动跑流水线 → `PUSHED` → 登记清理） |
| ✅ | **文案外置** —— `cmd.rs` 里 187 处固定输出全部走语言文件，zh-CN / en-US 各 328 个 key |
| P2 | **9 个内置插件本体** —— 用户已确认可暂缓（宿主可用，范例见 `plugins/example-echo/`） |
| P3 | 需求外功能 `overlay`（桌面悬浮窗）—— **用户已确认保留，不要再提议删除** |
| 观察 | 只能靠真机与人眼确认的项：PDF 出图、真实 QQ/微信推送、悬浮窗与托盘观感、连跑一周 |

## 提交信息怎么写

写清**为什么这么改**，不只是改了什么；踩到的坑要写进正文（HANDOFF 里的坑清单就是这样攒出来的）。

中文标点会让 PowerShell 解析坏 commit 命令，所以**把 message 写到文件再 `git commit -F`**：

```powershell
# 先写 _tmp/commit-msg.txt
git add -A; git commit -q -F _tmp/commit-msg.txt
```
