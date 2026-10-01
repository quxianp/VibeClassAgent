# VibeClassAgent 代码评审报告

评审范围：当前工作区源码、配置、依赖清单、README、HANDOFF、GUI 前端调用链、NapCat/OneBot 管理链路。评审只读完成，未修改源码、配置或测试文件。

## 1. 项目概述与理解

VibeClassAgent（VCA）是面向 Windows/希沃一体机的无人值守课堂录制与课后处理程序：`vca-cli` 负责 CLI/GUI 入口，`vca-gui` 通过 `tiny_http` 提供带 token 的本地 Web UI，`vca-engine` 负责按课表调度录制、处理窗口和清理，`vca-core` 提供课表、时间表、作业状态机、配置和持久化模型，`vca-platform` 封装 WASAPI/ffmpeg、转写、LLM、文档生成、HTTP、推送和 NapCat，`vca-plugin-host` 通过 JSON-RPC over stdio 运行外部插件。技术栈以 Rust 2021 为主，但当前流水线仍保留 Python Worker 路径；目标运行平台是 Windows。核心数据流是“时间表/课表 -> daemon -> CaptureSession -> JobStore -> Pipeline -> 推送 -> 到期清理”。

## 2. 问题总览

| 编号 | 位置 | 问题 | 严重程度 |
|---|---|---|---|
| R-01 | `crates/vca-engine/src/daemon.rs:172-180` | `lessons_today()` 固定传入 `WeekParity::Every`，`settings.term` 只用于日志，单双周课程实际不会按学期配置生效 | 严重 |
| R-02 | `crates/vca-core/src/config.rs:427-441`、`crates/vca-gui/src/api.rs:734-776` | 归一化和 GUI 列头始终使用 active/first 时间表，忽略 `ClassPlan.time_layout_id`，多份时间表会错排或显示错误时间 | 严重 |
| R-03 | `crates/vca-engine/src/daemon.rs:582-585,604-645,770-777` | 录制一开始就把作业标记为 `Recorded` 并指向最终 mp4；停止/收尾失败或进程停止时没有可靠完成 finalize 和路径回写 | 严重 |
| R-04 | `crates/vca-platform/src/napcat.rs:580-589` | 一键配置 OneBot HTTP 服务固定监听 `0.0.0.0`，且允许空 token，局域网可直接访问/调用机器人接口 | 严重 |
| R-05 | `crates/vca-plugin-host/src/runner.rs:138-148` | 插件调用使用阻塞 `read_line()`，超时只在读之前检查；插件不输出换行时会无限阻塞 | 严重 |
| R-06 | `crates/vca-plugin-host/src/manifest.rs:75-87`、`runner.rs:62-85` | manifest 的权限声明只是展示，未执行沙箱；任意本地插件可访问任意文件、网络和环境变量 | 严重 |
| R-07 | `crates/vca-core/src/paths.rs:227-230`、`store.rs:32-39,145-152` | profile 名未验证并直接拼入路径；命令行或配置可通过 `..` 逃出 profile/data 根目录 | 严重 |
| R-08 | `crates/vca-gui/src/server.rs:31-39`、`api.rs:636-686` | `UiSettings.allow_lan` 没有接入服务绑定逻辑，服务始终只监听回环；界面保存“允许局域网”后功能不生效 | 中等 |
| R-09 | `crates/vca-gui/src/api.rs:781-826` | `post_schedule()` 先写入配置，再返回 `issues`，验证失败的课表仍会持久化 | 中等 |
| R-10 | `crates/vca-gui/src/server.rs:127-134`、`api.rs:69-75` | API 业务错误统一以 HTTP 200 返回，客户端只能解析 JSON，监控/代理/脚本无法依赖 HTTP 状态码判断失败 | 中等 |
| R-11 | `crates/vca-gui/src/server.rs:153-162`、`api.rs:588-605` | 预览 token 用时间戳+PID 的非密码学混合生成；配置缺 token 时 `/preview/*` 完全不校验 | 中等 |
| R-12 | `crates/vca-gui/src/api.rs:1273-1325,1421-1452`、`vca-platform/src/napcat.rs:284-292` | NapCat 子进程句柄被丢弃且 PID 只存内存；VCA 重启后无法管理旧实例，重复启动可能产生多个 QQ 客户端/登录冲突 | 中等 |
| R-13 | `crates/vca-engine/src/pipeline.rs:265-277` | 崩溃恢复只检查 transcript/summary/docx，不检查 `shot_refs.json`、PDF、产物完整性；可能跳过损坏或缺失的中间产物 | 中等 |
| R-14 | `crates/vca-plugin-host/src/runner.rs:71-75`、`manifest.rs:114-119` | manifest 允许 `cdylib`/`wasm`，runner 却一律当可执行文件启动；声明模型与实际运行模型不一致 | 中等 |
| R-15 | `crates/vca-plugin-host/src/market.rs:143-153`、`manifest.rs:105-119` | 插件入口未校验为相对路径且未拒绝 `..`；force 安装先删除目标再递归复制，复制失败会留下半安装目录 | 中等 |
| R-16 | `crates/vca-platform/src/push.rs:408-430` | OneBot 文本成功、附件失败仍返回整体成功，后续作业进入 `Pushed` 并允许清理，附件可能永久丢失 | 中等 |
| R-17 | `crates/vca-engine/src/daemon.rs:517-523,582-585` | 录制作业创建/状态更新失败只记录日志并继续，可能出现已经产生录制文件但无可恢复 Job 记录的孤儿数据 | 轻微/中等 |
| R-18 | `README.md:13-15,195-216`、`HANDOFF.md:7,15,114-140` | 文档中的测试数量、提交号、源码规模、Python/纯 Rust、推送渠道等信息互相矛盾或已过期，容易误导后续维护者 | 中等 |
| R-19 | `crates/vca-engine/src/daemon.rs:771-777` | daemon 退出路径只 `stop()` 不 `finalize()`，正常通过 GUI/托盘停止时可能留下未封装的临时视频和未更新的作业 | 严重 |
| R-20 | `crates/vca-gui/src/api.rs:16-25` | POST body 无大小上限且忽略读取错误；本机其它进程可发送超大请求造成内存压力或得到不完整 JSON | 轻微/中等 |

## 3. 详细问题清单

### 严重问题

#### R-01：单双周配置没有进入实际排课

- **位置**：`crates/vca-engine/src/daemon.rs:172-180`，特别是 `WeekParity::Every`。
- **问题**：`TermConfig::is_odd_week()` 在 `daemon.rs:376-390` 只输出日志，`lessons_today()` 没有根据日期计算 `Odd/Even`，而是始终把 `Every` 传给 `schedule::plan_for_date()`。
- **影响**：带 `cycle: 单周/双周` 或 `TimeRule.week_count` 的课程不会按真实学期周过滤。双周课程可能每周录，单双周错课可能被录入错误作业；这直接影响录制、处理和推送。
- **严重程度**：严重。
- **修改建议**：在 `lessons_today()` 内调用 `settings.term.is_odd_week(date)`；配置存在时转换为 `WeekParity::Odd/Even`，未配置时明确采用自然周回退策略并在状态页显示警告。增加固定日期的单周/双周测试，覆盖学期前日期、首周为双周、跨年和周一边界。

#### R-02：ClassPlan 的时间表绑定被忽略

- **位置**：`crates/vca-core/src/config.rs:427-441`；`crates/vca-gui/src/api.rs:734-776`、`836-864`。
- **问题**：`ScheduleFile::normalize()` 选择 `timetables.iter().find(|t| t.is_active).or(first)`；GUI 获取课表的 `class_slots` 也按 active/first 选择。`ClassPlan.time_layout_id` 只在保存时检查“是否存在”，没有用于查找实际时间表。
- **影响**：当用户有多份时间表且课表绑定的不是 active 表时，period 会被补成另一份时间表的时间；GUI 列头和 daemon 排课不一致，导致错录/漏录。
- **严重程度**：严重。
- **修改建议**：抽出 `resolve_timetable(plan, timetables) -> Result<Option<&Timetable>>`：非空 `time_layout_id` 必须按 ID 解析；为空时才使用 active/first 兼容旧配置。`normalize()`、`get_schedule()`、`post_schedule()`、daemon 加载和导入都调用同一解析函数。绑定 ID 不存在时拒绝保存或明确进入不可运行状态，不能继续静默使用另一份时间表。

#### R-03/R-19：录制生命周期与作业持久化不一致

- **位置**：`crates/vca-engine/src/daemon.rs:582-585`、`604-645`、`770-777`；`crates/vca-platform/src/capture.rs:628-768`。
- **问题**：开始 ffmpeg 后立即把 Job 标为 `Recorded` 并写入 `video_path`；最终文件只有 `CaptureSession::finalize()` 成功后才产生。正常退出路径只调用 `stop()`，没有 `finalize()`，也没有把 `audio_path`/最终 `video_path` 回写 Job。收尾失败时仍可能保留指向不存在最终文件的路径。
- **影响**：GUI/托盘停止、系统关闭或 daemon 异常时，Job 看起来可处理，但 pipeline 找不到有效媒体；临时视频、WAV 和截图可能无人管理。无人值守运行中断后会形成“已录制但永远无法恢复”的作业。
- **严重程度**：严重。
- **修改建议**：把状态转换拆成 `Recording`/`Recorded`，或至少在 `finalize()` 成功后才推进到 `Recorded`。停止路径统一执行 `stop -> finalize -> 回写 Job`，包括正常退出、录制时段结束、异常退出清理。最终文件存在且可读后才设置 `video_path`；失败时保留 raw/audio 路径并进入可重试状态。增加“daemon 在录制中退出后重启”的集成测试。

#### R-04：NapCat OneBot 服务暴露到整个局域网

- **位置**：`crates/vca-platform/src/napcat.rs:580-589`。
- **问题**：`ensure_http_server()` 写入 `"host": "0.0.0.0"`，同时 `token` 由 UI 输入且可以为空。OneBot HTTP 服务因此可能监听所有网卡，空 token 时没有认证。
- **影响**：同一校园网中的设备可能调用 `/send_group_msg`、上传文件或读取登录信息；QQ 机器人凭借该接口可被未授权发送消息。校园网不是可信边界，尤其不应把凭据为空作为可接受配置。
- **严重程度**：严重。
- **修改建议**：默认绑定 `127.0.0.1`；只有用户明确开启 LAN 访问时才绑定指定局域网地址，并强制生成高熵 token。空 token 时拒绝启用 HTTP 服务。配置保存前做 host/port/token 校验；状态页显示监听地址和认证状态。不要使用时间戳+PID 作为长期 token。

#### R-05：插件调用的超时实际上可能失效

- **位置**：`crates/vca-plugin-host/src/runner.rs:138-148`。
- **问题**：循环前检查 `started.elapsed()`，但 `BufReader::read_line()` 是阻塞调用。插件启动后保持 stdout 打开但不输出换行时，线程会永久卡在 `read_line()`，无法回到超时判断。
- **影响**：一个失控插件可以卡住整个课后流水线；daemon 在处理窗口内无法继续处理其它作业，也无法响应“停止/退出”到下一安全点。
- **严重程度**：严重。
- **修改建议**：把 stdout 读取放到独立 reader 线程，通过带超时的 channel 接收完整行；超时后杀死子进程并回收句柄。Windows 可使用独立进程+管道线程，不要在业务线程直接阻塞。补充“不输出、输出半行、输出非 JSON、进程退出”的测试。

#### R-06：插件权限声明不是安全隔离

- **位置**：`crates/vca-plugin-host/src/manifest.rs:75-87`、`runner.rs:71-85`；`crates/vca-plugin-host/src/market.rs:155-161`。
- **问题**：manifest 只把 `network/filesystem/env` 返回给 UI 展示，runner 启动普通进程，没有 Windows Job Object、ACL、网络限制或环境变量白名单。任意本地插件可读取用户文件、凭据和网络，并不受声明限制。
- **影响**：用户一旦安装恶意或被篡改的插件，`secrets.env`、模型 key、QQ token 等都可被读取；“权限清单”会产生错误的安全预期。
- **严重程度**：严重。
- **修改建议**：短期在文档和 UI 中明确“权限声明仅为告知，不是沙箱”；安装前要求明确确认并禁止自动信任来源。若要宣称隔离，必须实际实现：最小化环境变量、工作目录/文件 ACL、Windows Job Object、低权限 token、网络白名单，或改用 WASI/受限容器。未实现前不要把 `process` 描述为安全隔离。

#### R-07：profile 路径缺少边界校验

- **位置**：`crates/vca-core/src/paths.rs:227-230`、`crates/vca-core/src/store.rs:32-39,145-152`；入口 `crates/vca-cli/src/main.rs:259-275`。
- **问题**：profile 直接进入 `data_root/profiles/{profile}`，没有限制为空、`.`、`..`、路径分隔符、绝对路径或 Windows 保留名。`job_id_for()` 虽清洗课程名，但最后直接拼接未校验的 profile。
- **影响**：恶意命令行参数或被篡改的启动配置可能读写 data/config 根目录之外的文件，破坏多教师隔离，甚至覆盖任意可写路径下的 `job.json`。
- **严重程度**：严重。
- **修改建议**：增加统一 `validate_profile_name()`：只允许 ASCII 字母、数字、`_-.`，拒绝 `..`、路径分隔符、冒号、空值和 Windows 保留名；所有 CLI、GUI、JobStore 构造路径前调用。更稳妥的做法是把 profile 转为内部安全 ID，显示名与目录名分离。增加 Windows 路径测试。

### 中等问题

#### R-08：允许局域网设置没有接入监听器

- **位置**：`crates/vca-core/src/config.rs:315-335`、`crates/vca-gui/src/api.rs:677-686`、`crates/vca-gui/src/server.rs:31-39`。
- **问题**：GUI 可以保存 `ui.allow_lan`，但 `ServeOptions` 没有该字段，`serve()` 永远 `TcpListener::bind(("127.0.0.1", opts.port))`。保存接口只提示要重启，却没有任何重启后读取配置并绑定 LAN 的逻辑。
- **影响**：用户打开该开关后预览仍无法从手机访问；更严重的是配置模型和实际安全边界不一致，后续维护者可能误以为已暴露或已保护。
- **严重程度**：中等。
- **修改建议**：明确设计：默认只绑 loopback；允许 LAN 时读取配置并绑定显式地址，配合预览 token 和 OneBot 的独立认证。将绑定地址显示在状态页，并增加真实 socket 绑定测试。若暂不实现，应删除设置项与文案，避免伪功能。

#### R-09：课表验证在写盘之后执行

- **位置**：`crates/vca-gui/src/api.rs:781-826`。
- **问题**：函数先 `serde_yaml::to_string` + `std::fs::write`，再回读并计算 `issues`。重叠时间、缺失绑定时间表等问题只放到响应 JSON，不会阻止错误配置落盘。
- **影响**：用户看到保存成功但 daemon 后续可能错排；“issues”不是阻断式校验，界面若未展示或用户忽略，就会把坏配置带入无人值守运行。
- **严重程度**：中等。
- **修改建议**：先在内存中 normalize/validate；存在结构性错误时返回 4xx/`ok:false` 且不写文件。通过校验后再归档并原子替换；归档和新文件都应使用临时文件+rename，避免断电产生半文件。

#### R-10：业务失败返回 HTTP 200

- **位置**：`crates/vca-gui/src/api.rs:69-75`、未知接口 `:66`。
- **问题**：Rust 错误和业务失败都序列化为 `{ok:false}`，但使用 `respond_json(req, 200, ...)`；未知 API 也返回 200。
- **影响**：浏览器前端可以适配，但脚本、监控、反向代理和未来移动端无法用标准 HTTP 语义处理错误；日志和网络调试容易误判为成功。
- **严重程度**：中等。
- **修改建议**：定义错误映射：400 参数错误、401/403 token、404 路由/资源、409 状态冲突、500 内部错误。保留 JSON `ok/error` 作为兼容字段。前端统一处理非 2xx。

#### R-11：预览令牌生成和缺省校验不安全

- **位置**：`crates/vca-gui/src/server.rs:322-338`、`crates/vca-gui/src/api.rs:588-605`、`server.rs:153-162`。
- **问题**：短期 token 和长期 preview token 都是时间戳、PID 和固定乘法混合，不是密码学随机数；同时 `preview_token` 为空时，预览路由完全跳过校验。
- **影响**：在未来启用 LAN 访问时，攻击者可猜测或撞库 token；当配置写入失败/旧配置为空时，预览内容可能无认证暴露。
- **严重程度**：中等。
- **修改建议**：使用操作系统 CSPRNG 生成至少 128 bit token；预览路由在 allow_lan 为真时强制 token 非空，否则拒绝服务而不是放行。不要把 token 以明文回显到不必要的 API 响应或日志中。

#### R-12：NapCat 重启后无法复用/管理已登录实例

- **位置**：`crates/vca-gui/src/api.rs:1268-1325,1421-1452`、`crates/vca-platform/src/napcat.rs:284-292`。
- **问题**：NapCat 的 `Child` 被 `drop`，只保存当前 VCA 进程内的 PID；VCA 重启后 `NAPCAT_PID` 为 `None`，状态只靠端口/配置判断，启动按钮不会识别并复用已有进程，也不能安全停止它。
- **影响**：VCA 重启、Windows 更新或 GUI 重开后可能重复启动 NapCat，导致多个 QQ 登录实例、端口冲突、登录风控；用户也会误以为每次都要重新登录。
- **严重程度**：中等。
- **修改建议**：使用固定 NapCat 数据目录，不删除 `NapCat.Shell`/QQ 的登录缓存；通过 OneBot `get_login_info` 和配置端口检测已有实例。启动前若端口已是当前 QQ 的 OneBot 服务，应复用而不是再启动；若需管理进程，记录 pid 文件并校验进程映像路径后再停止，不能只信任 pid。启动器应保留 `Child` 或使用受控 supervisor，处理崩溃重启和退出回收。

#### R-13：流水线恢复检查不完整

- **位置**：`crates/vca-engine/src/pipeline.rs:265-277`。
- **问题**：`first_incomplete_state()` 只检查 `transcript.txt`、`summary.json` 和 `docx_path`。它没有检查 `transcript.json`、`shot_refs.json`、PDF 路径、文件存在性和内容可解析性。
- **影响**：断电/磁盘损坏后，存在空文件或旧路径时可能跳过必要步骤，生成缺截图、缺 PDF 或使用旧结果的文档；恢复行为不符合注释中的“从最后成功状态继续”。
- **严重程度**：中等。
- **修改建议**：按每个状态定义完整产物契约，并用“存在 + 可解析 + 非空 + 属于当前 job”判断。把产物清单或阶段 manifest 写入 job 目录，恢复时依据 manifest，而不是散落的 `is_file()` 和 Option 字段。

#### R-14：插件 runtime 类型与实际执行不一致

- **位置**：`crates/vca-plugin-host/src/manifest.rs:114-119`、`runner.rs:62-85`。
- **问题**：manifest 接受 `process`、`cdylib`、`wasm` 三种字符串，但 runner 只执行 `Command::new(entry)`。选择 `cdylib`/`wasm` 的插件会被当作普通可执行文件启动，错误信息不清晰。
- **影响**：插件作者按清单声明的运行模式无法工作，或者把非可执行文件交给系统执行；接口契约容易产生错误信任。
- **严重程度**：中等。
- **修改建议**：v1 若只支持 process，就在 manifest 校验中拒绝其他类型并删除未实现枚举；若要保留扩展点，为每种类型实现明确加载器和能力边界，不能先接受再在运行时失败。

#### R-15：插件安装缺少入口路径校验和事务性替换

- **位置**：`crates/vca-plugin-host/src/market.rs:143-153`、`manifest.rs:105-119`。
- **问题**：`plugin_dir.join(manifest.runtime.entry)` 没有确认入口解析后仍在插件目录内；force 安装先删除目标目录，再递归复制，复制失败会留下不完整安装。
- **影响**：恶意 manifest 可引用目录外入口；安装中断后宿主发现半安装插件，后续启动错误或丢失旧版本。
- **严重程度**：中等。
- **修改建议**：只允许相对路径，拒绝绝对路径和 `..`；canonicalize 后校验前缀。force 安装复制到临时目录，完成 manifest/文件校验后原子 rename，并保留旧版本回滚目录。

#### R-16：附件上传失败被当作推送成功

- **位置**：`crates/vca-platform/src/push.rs:408-430`。
- **问题**：OneBot 先发文本，再“尽力而为”上传附件；上传失败只 warning，最终返回 `PushOutcome::ok`。Job 因此进入 `Pushed`，清理策略可以删除本地录像/文档。
- **影响**：用户看到消息但没有 Word/PDF，且到期清理后附件无法重发。注释中的“推送成功才清理”实际只保证文本成功，不保证用户要求的完整文档成功。
- **严重程度**：中等。
- **修改建议**：把文本和附件结果建模为 `TextSent/AttachmentSent/Partial`；默认 partial 不进入 `Pushed`，或增加用户明确选择的“文本成功即算成功”策略。清理前必须确认所需附件都成功或有可恢复的上传记录。

#### R-17：录制时 Job 持久化失败没有恢复登记

- **位置**：`crates/vca-engine/src/daemon.rs:517-523,582-585`。
- **问题**：`ensure_for_lesson` 失败时下一轮重试；录制启动后 `self.store.save(&j)` 的错误却被直接忽略。此时 ffmpeg 已经产生文件，但没有可靠 Job ID/路径记录。
- **影响**：录制文件成为无法在 GUI/清理/流水线中发现的孤儿文件，长期占用磁盘，且用户无法手动重试。
- **严重程度**：轻微/中等。
- **修改建议**：保存失败时立即停止并 finalize 当前录制，或写入独立 recovery journal（包含路径、课程、时间）供下次启动扫描。不要无条件忽略保存错误；至少在状态页显示孤儿录制数量。

### 轻微和维护性问题

#### R-18：文档与源码状态漂移

- **位置**：`README.md:13-15,195-216`、`HANDOFF.md:7,15,114-140`、各模块顶部注释。
- **问题**：README 声称纯 Rust，但 `vca-engine/src/worker.rs` 和 pipeline 仍保留 Python Worker；README/HANDOFF 的测试数量、提交号、源码规模、推送渠道和“已实现”状态互相不一致；`HANDOFF` 还写“六种推送渠道”，当前 `Provider` 枚举是六项但其中包含 dry-run，文档口径混杂。
- **影响**：后续 agent 会依据错误文档做判断，容易重复实现、漏测或误删仍在用的 Worker。对无人值守产品，错误的运行说明会直接导致部署失败。
- **严重程度**：中等（工程风险）。
- **修改建议**：把测试/源码规模等动态数字改成 CI 生成或删除；每次发布由脚本更新版本、commit 和能力表。明确“Rust 主程序 + 可选 Python Worker”与“发布包是否包含 Python runtime”的真实状态。新增文档一致性检查，扫描过时 provider/路径/测试数字。

#### R-20：GUI 请求体没有上限

- **位置**：`crates/vca-gui/src/api.rs:15-25`。
- **问题**：对所有 POST 直接 `read_to_string`，不检查 `Content-Length`、请求大小或读取错误。
- **影响**：即使服务只监听本机，任意本机进程也可发送超大 body 消耗内存；导入 ClassIsland JSON 也没有单独大小限制。
- **严重程度**：轻微/中等。
- **修改建议**：统一限制 body，例如普通配置 1 MiB、ClassIsland 导入按明确上限放宽；读取失败立即返回 400；超过上限时提前关闭请求并记录来源。

## 4. 精简与微重构建议

以下建议不是为了重写架构，而是减少重复逻辑和隐式约定：

1. **统一时间表解析**：新增一个 `resolve_timetable_for_plan()`，替代 `normalize()`、GUI `get_schedule()`、daemon 的 active/first 三套选择逻辑。这样可同时解决 R-02 和减少重复代码。
2. **统一录制结束事务**：把 `stop + finalize + Job 回写` 收成 daemon 的一个函数，正常下课、手动停止和进程退出全部复用。不要让不同路径分别决定是否 finalize。
3. **给 Job 建立阶段产物清单**：用一个小型 `JobArtifacts`/manifest 记录 transcript、segments、summary、shots、docx、pdf，替代 pipeline 中散落的路径判断。
4. **插件 v1 只保留 process**：当前 runner 只实现进程模式，删除未实现 `cdylib/wasm` 声明，等真正有运行时再扩展，避免虚假能力。
5. **HTTP 错误映射集中化**：在 GUI server/API 层定义 `ApiError` 到状态码的映射，不要每个接口返回手工 JSON 200。
6. **统一安全路径函数**：profile、job id、plugin id 都通过同一个安全组件生成目录名；业务层禁止直接 `PathBuf::join(user_input)`。
7. **删除或实现 allow_lan**：如果短期没有可靠的 LAN 绑定与认证，就删除该开关；若保留，必须把监听地址、preview token、OneBot host/token 一起纳入同一套安全配置。
8. **减少长文件职责混合**：`vca-gui/src/api.rs` 已超过 1600 行，建议只做按资源拆分：`api/config.rs`、`api/schedule.rs`、`api/napcat.rs`、`api/jobs.rs`，不改变公共入口。
9. **清理过期注释**：例如 `Pipeline`/`Daemon` 中仍描述 Python Worker、`HANDOFF` 中仍写旧测试数量；注释应该描述当前代码而不是历史设计。

## 5. 个人 QQ Bot 本地持久登录方案

有方案，但它不是“保存 QQ 密码后自动登录”这么简单；应依赖机器人实现自己的设备/会话缓存，并接受平台风控和协议风险。

### 推荐方案：NapCat Shell + 固定数据目录

当前代码已经把 NapCat 安装在 `<tools>/napcat/`，但 `napcat.rs:247-292` 只负责启动入口，并没有显式管理登录数据目录。实际部署时应：

1. 固定 NapCat/QQ 的安装根目录，不要每次更新时删除或覆盖其配置、设备信息和登录缓存目录。
2. VCA 启动前先检测 OneBot HTTP 端口并调用 `get_login_info`；已有登录实例就复用，不再次拉起 QQ。
3. 只有检测到服务不存在时才启动 NapCat；保留 supervisor/pid 文件，重启后校验 PID 对应的可执行文件路径。
4. 二次验证、滑块、风控或设备变更仍可能要求重新登录；不能承诺永久免扫码。
5. OneBot HTTP 服务必须只绑定 `127.0.0.1`，并强制设置 token；若确需局域网访问，使用防火墙白名单和高熵 token。

NapCat Shell 的启动形态与登录方式可参考：[NapCat Shell 文档](https://doc.napneko.icu/guide/boot/Shell.html)。登录缓存的具体文件名和目录属于上游实现细节，不应由 VCA 自己复制或伪造。

### 可替代方案：Lagrange.OneBot

Lagrange 通常把设备信息、登录相关状态和签名/设备配置放在用户指定的数据目录。重点是让其配置目录在 VCA 的持久化 `tools/napcat` 或独立 `data/qqbot` 下，并在升级时保留该目录；同时按文档配置签名服务/设备信息，避免每次当成新设备登录。

参考：[Lagrange 创建 Bot 实例](https://lagrangedev.github.io/Lagrange.Doc/v1/Lagrange.Core/CreateBot/)、[Lagrange 快速部署与配置](https://lagrangedev.github.io/Lagrange.Doc/v1/Lagrange.OneBot/Config/)、[Lagrange 登录扩展](https://lagrangedev.github.io/Lagrange.Doc/v1/Lagrange.Core/Login/Extern)。具体配置字段应以当前版本文档和实际生成的配置为准，不能把某个版本的缓存文件名硬编码到 VCA。

### 对当前代码的结论

当前实现**不能保证重启 VCA 后不重新登录**：

- `napcat::start()` 在 `:284-292` 丢弃 `Child`，只返回 PID；
- `api.rs` 的 `NAPCAT_PID` 是进程内静态变量，VCA 重启后为空；
- `bot_start()` 只检查当前进程保存的 PID，不会先通过端口和 `get_login_info` 复用旧实例；
- `ensure_http_server()` 还会把 host 写为 `0.0.0.0`，这应先修复为本机绑定和强制 token。

因此，最小可行改造是“固定数据目录 + 启动前探测并复用 + 进程监督 + 不删除登录缓存”，而不是把 QQ 密码写入 VCA 配置。任何 token、密码、设备密钥都必须继续放在环境变量或被 `.gitignore` 排除的本地 secrets 文件中。

## 6. 建议修改顺序

### P0：必须先修

1. 修 R-04：OneBot 改为 loopback 默认、强制 token、LAN 显式开启并有防护。
2. 修 R-07：profile/job/plugin 路径边界校验，补 Windows 路径测试。
3. 修 R-05/R-06：插件超时不能阻塞；在未实现沙箱前降低权限声明的安全表述，或实现真正隔离。
4. 修 R-03/R-19：统一录制 stop/finalize/Job 回写事务，确保中断后可恢复。
5. 修 R-01/R-02：让学期单双周和 `time_layout_id` 真正参与 daemon 排课。

### P1：建议随后修

1. 修 R-09：校验通过后再写课表，采用原子写和归档。
2. 修 R-11/R-12：使用 CSPRNG token；NapCat 启动前探测并复用已有实例，固定持久化登录目录。
3. 修 R-13：建立完整阶段产物契约，避免恢复时跳过坏产物。
4. 修 R-14/R-15：只支持已实现的插件 runtime；入口路径校验和事务性安装。
5. 修 R-16：附件部分失败不得静默推进到可清理的 `Pushed`。
6. 修 R-08/R-10/R-20：实现或删除 LAN 开关、规范 HTTP 状态码、限制请求体。

### P2：工程维护

1. 统一动态文档和测试数字，清理过期注释与能力描述。
2. 按资源拆分超长 `vca-gui/src/api.rs`，保持公共 dispatch 不变。
3. 补充真实 Windows 集成测试：录制中退出、重启恢复、NapCat 已登录复用、LAN 绑定、插件无输出超时、profile 路径攻击。
4. 增加依赖审计流程，例如固定版本更新策略和 `cargo deny`/等效漏洞扫描；本次评审未对 crates.io 当前漏洞状态作在线断言，需在 CI 网络环境中另行执行。

## 7. 评审限制与待确认项

- 本次没有使用真实 QQ/OneBot 账号，也没有验证 NapCat 当前版本的具体登录缓存文件名；上述持久登录建议依据上游文档和“保留同一数据目录”的通用机制，具体字段需在目标版本实测。
- 本次没有执行一周无人值守运行，也没有验证真实 GUI 停止/系统关机时 ffmpeg 与 WASAPI 的所有退出竞态。
- 依赖版本已锁在 `Cargo.lock`，但本次没有联网执行漏洞数据库扫描；不能据此声明“无已知漏洞”。
- `R-03/R-19` 的影响基于 `CaptureSession::finalize()` 负责生成最终输出、daemon 退出只调用 `stop()` 的源码路径；应通过录制中调用 GUI/托盘停止的集成测试确认具体遗留文件集合。
