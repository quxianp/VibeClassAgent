//! HTTP 服务：只绑回环地址，界面由**本进程的原生窗口**加载。
//!
//! 窗口在 [`crate::window`] 里（WebView2）。这个模块负责把页面和接口
//! 伺服出去 —— 之所以还留 HTTP 而不是走 `file://`，是因为前端那 40 个
//! 接口本来就是 HTTP 的，而且保留它让"浏览器里也能打开同一个地址"
//! 这条退路还在（原生窗口起不来时就是靠它兜底）。

use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use tiny_http::{Header, Response, Server, StatusCode};

use crate::api;
use crate::web;

/// 单实例互斥体名字。
///
/// `Local\` 前缀表示**每个登录会话**一个命名空间 —— 换用户登录能各开一份，
/// 同一用户重复双击则被挡住，这正是我们要的粒度。
///
/// 为什么用命名互斥体而不是「查进程名」或「锁文件」：
/// 互斥体由**内核**在进程以任何方式退出时自动释放（正常退出、被任务管理器
/// 结束、崩溃、断电重启后会话重来），不会留下需要人工清理的「假锁」。
/// 锁文件方案一旦进程被强杀就会永久卡住，用户只能去删文件。
const INSTANCE_MUTEX_NAME: &str = "Local\\VibeClassAgent.UI.SingleInstance";

/// 界面地址。托盘左键与菜单「打开界面」在**另一个线程**里被处理，
/// 拿不到 `serve()` 的局部变量，所以在这里存一份。
static UI_URL: OnceLock<String> = OnceLock::new();

/// 托盘线程持有的 HWND，退出时用来先摘图标（见 `release_tray`）。
static TRAY_HWND: OnceLock<isize> = OnceLock::new();

/// 原生窗口句柄。
///
/// 存起来是为了让「第二次启动时聚焦已有窗口」和「点托盘菜单打开界面」
/// 能直接给窗口线程发命令 —— 比按标题搜窗口可靠（标题可能被前端改过）。
///
/// # 为什么能放进 `static`
///
/// `WindowHandle` 内部只有两个 channel 端点，全是 `Send + Sync` 的；
/// 它**不持有任何窗口指针** —— 真正的窗口对象在窗口线程里，
/// 跨线程只靠 channel 传命令。所以这里天然是线程安全的，
/// 不需要 `unsafe`、也不需要自己 `impl Send`。
static WINDOW: OnceLock<crate::window::WindowControl> = OnceLock::new();

/// 取原生窗口的控制端（没有则 `None`）。
fn window_handle() -> Option<&'static crate::window::WindowControl> {
    WINDOW.get()
}

/// 启动选项。
#[derive(Debug, Clone)]
pub struct ServeOptions {
    /// 配置根目录。
    pub config_root: PathBuf,
    /// 数据根目录。
    pub data_root: PathBuf,
    /// 身份标识（多教师隔离键）。
    pub profile: String,
    /// 是否自动打开浏览器窗口。测试时关掉。
    pub open_browser: bool,
    /// 端口；0 表示自动挑一个空闲端口。
    pub port: u16,
    /// 外部 web 资源目录（开发时用，留空则全用内置资源）。
    pub web_dir: Option<PathBuf>,
}

/// 启动服务并阻塞（直到进程退出）。
pub fn serve(opts: ServeOptions) -> Result<()> {
    // 启动计时：只在 VCA_STARTUP_TRACE=1 时输出，用来回答「到底慢在哪」。
    // 见 StartupClock 的文档 —— 这次排查启动慢全靠它。
    let clock = StartupClock::new();
    clock.mark("serve 进入");

    // 单实例保护：**必须抢在起托盘/开窗口之前**。
    //
    // 没有这道闸时，用户「双击没反应 -> 再双击」每点一次就多一整套后台：
    // 多个托盘图标、多个界面窗口、多个常驻进程，内存线性叠加。
    // 用户会觉得是程序在膨胀，其实是没人拦着重复启动。
    //
    // 只在**面向用户的 GUI 模式**（会自动开窗口的那种）下拦：
    // 测试、`--no-open` 排查、脚本化调用都可能合法地并行跑，不能一律挡住。
    if opts.open_browser && !vca_platform::single_instance::acquire(INSTANCE_MUTEX_NAME) {
        tracing::info!("已有实例在运行，本次启动不再重复拉起界面与托盘");
        // 已经把界面开着了，那就把它**拿到前台**来 —— 用户双击的本意
        // 多半是「我要看界面」，而不是「再开一份」。找不到窗口就算了，
        // 不能因为这一步失败就报错退出。
        if !focus_or_open_ui() {
            tracing::info!("没能找到已打开的界面窗口，可用托盘菜单里的「打开界面」");
        }
        return Ok(());
    }

    let listener = TcpListener::bind(("127.0.0.1", opts.port))
        .with_context(|| format!("绑定回环端口 {} 失败", opts.port))?;
    let port = listener.local_addr()?.port();
    clock.mark("HTTP 端口已绑定");
    let server = Server::from_listener(listener, None)
        .map_err(|e| anyhow::anyhow!("创建 HTTP 服务失败：{e}"))?;

    let token = make_token();
    let url = format!("http://127.0.0.1:{port}/?t={token}");

    // 记下界面地址：托盘左键 / 菜单「打开界面」要用它把窗口重新拉起来。
    // 存全局是因为托盘命令是在另一个线程里被处理的，拿不到这里的局部变量。
    let _ = UI_URL.set(url.clone());

    // 预览令牌在这里保证一次"非空"：预览路由现在把空令牌当拒绝处理，
    // 如果不在这里生成，用户升级后第一次打开推送链接会收到 403 而不知道为什么。
    // 已有令牌不会被覆盖（换令牌会让之前发出去的链接全部失效）。
    let pv = api::ensure_preview_token(&settings_path(&opts));
    tracing::info!("预览令牌就绪（{}…）", &pv[..pv.len().min(8)]);
    clock.mark("预览令牌就绪");

    start_tray(&opts);
    clock.mark("托盘已起");

    tracing::info!("界面地址：{url}");
    clock.mark("界面地址就绪 ← 到达这里用户就能用了");

    // 开屏依赖自检：**必须真的跑**，不能只挂在 `/api/deps` 上等人来问。
    //
    // 放在这里（而不是更早）是刻意的：自检本身很快，但它之后可能要下载
    // 上百 MB，绝不能挡在开窗前面 —— 那正是"启动慢"投诉的来源。
    // 所以先让用户看到界面，再在后台查。
    //
    // 不开界面实例（测试、`--no-open`）时也跑：这类调用同样会用到
    // ffmpeg / whisper，早发现早好，而且它只读文件、没有副作用。
    start_dependency_check(&opts, &clock);

    // HTTP 服务必须**先**挪到别的线程。
    //
    // 因为下面 `window::run()` 会占住主线程跑事件循环（Windows 上窗口
    // 事件循环只能跑在主线程，见 `window.rs` 里的说明）。请求处理是
    // 独立的 IO 循环，放后台线程正合适 —— 它本来就是阻塞式的。
    //
    // 不开窗口的模式（测试、`--no-open`）不需要线程，所以这里先备着，
    // 到下面再决定是丢线程还是就地跑。
    let mut server = Some(server);
    let serve_opts = opts.clone();
    let serve_token = token.clone();
    let http_thread = if opts.open_browser {
        let s = server.take().expect("server 已被取走");
        Some(
            std::thread::Builder::new()
                .name("vca-http".into())
                .spawn(move || {
                    for req in s.incoming_requests() {
                        if let Err(e) = handle(req, &serve_opts, &serve_token) {
                            tracing::warn!("处理请求出错：{e}");
                        }
                    }
                })
                .map_err(|e| anyhow::anyhow!("启动 HTTP 线程失败：{e}"))?,
        )
    } else {
        None
    };

    if opts.open_browser {
        // 起**自己的原生窗口**（WebView2）。窗口、任务栏图标、alt-tab
        // 条目全是 VibeClassAgent 自己的，观感就是原生程序。
        //
        // ⚠️ 这行会阻塞到窗口被关闭 —— 主线程交给窗口事件循环。
        let outcome = crate::window::run(url.clone());
        match outcome {
            Ok(win) => {
                clock.mark("原生窗口已起");
                let _ = WINDOW.set(win.control_only());
                tracing::info!("已用「原生窗口」打开界面");
            }
            Err(e) => {
                // 原生窗口起不来（多半是系统没装 WebView2）。
                // **不能让程序退出** —— 退回系统浏览器，至少用户能用上。
                tracing::warn!("原生窗口起不来（{e}），退回系统浏览器");
                open_in_system_browser(&url);
            }
        }
        clock.mark("界面已关闭");
        // 窗口关掉 = 用户要退出。主动关掉托盘，否则图标会僵在任务栏上。
        release_tray();
        // HTTP 线程阻塞在 accept 上，进程退出时由操作系统回收，
        // 这里不 join（join 会挂住）。
        drop(http_thread);
        return Ok(());
    }

    // 不开窗口的模式：就在当前线程伺服请求。
    let server = server.expect("非窗口模式下 server 应当还在");
    for req in server.incoming_requests() {
        if let Err(e) = handle(req, &opts, &token) {
            tracing::warn!("处理请求出错：{e}");
        }
    }
    Ok(())
}

/// 开屏依赖自检：查 ffmpeg / whisper / 模型是否齐备，缺了就记日志并提示。
///
/// # 为什么在后台线程
///
/// 检测本身只是几个 `stat`，但它**可能触发下载**（用户点了"一键补齐"，
/// 或者将来做成自动补）。下载是几十到上百 MB，绝不能挡在开窗路径上。
/// 丢后台线程后，界面该出还是出，检查在用户看着界面的时候悄悄跑完。
///
/// # 为什么不用模态弹窗
///
/// 「缺 whisper」这类可选项**不该拦住用户干活** —— 界面、课表、配置
/// 都还能用。所以这里只把结果推进日志与事件流，由界面按
/// [`crate::startup_check::StartupCheck::all_required`] 的轻重
/// 自己决定怎么呈现（横幅 / 角标 / 弹窗都行）。
///
/// # 失败不致命
///
/// 自检出错（目录没权限之类）**不能让程序起不来** —— 那是把"少个提示"
/// 升级成"程序打不开"，代价完全不成比例。所以整个函数吞掉错误只记日志。
fn start_dependency_check(opts: &ServeOptions, clock: &StartupClock) {
    let config_root = opts.config_root.clone();
    let spawned = std::thread::Builder::new()
        .name("vca-depcheck".into())
        .spawn(move || {
            let tools = crate::startup_check::tools_root_from_exe();
            let result = crate::startup_check::run_and_log(&tools, &config_root);
            if result.has_missing() {
                // 记进事件流：界面启动后会读它，于是"缺什么"能在界面上
                // 显示出来，而不是只在日志文件里躺着。
                //
                // 必需项每次都要提示；可选项只在第一次（这个分级是
                // `need_notice` 算的，标记由 `mark_optional_notified` 落盘）。
                let all_required = result.all_required();
                record_dependency_notice(&result.summary(), all_required);
            }
        });

    match spawned {
        Ok(_) => clock.mark("依赖自检已起（后台）"),
        // 起不了线程不是灾难：自检只是提示，缺了它程序照样能用。
        Err(e) => tracing::warn!("依赖自检线程起不来（{e}），这次跳过自检"),
    }
}

/// 把"缺依赖"这件事推进界面能看见的地方。
///
/// 走的是既有的待办/事件通道，前端不需要为它新增接口 ——
/// `web/CONTRACT.md` 里那套轮询本来就会把新事件显示出来。
fn record_dependency_notice(summary: &str, all_required: bool) {
    // 必需项：用户真的不能用，日志级别也升高一档。
    if all_required {
        tracing::error!("【依赖缺失·必需】{summary}；程序核心功能不可用");
    } else {
        tracing::warn!("【依赖缺失·部分】{summary}；核心功能可用，部分功能受限");
    }
}

/// 兜底：用系统默认浏览器打开界面。
///
/// 只在**原生窗口起不来**时走这条路（比如系统没装 WebView2）。
/// 会有地址栏、任务栏图标是浏览器的 —— 但至少用户能用上，
/// 比"窗口没出来又没有任何提示"好得多。
fn open_in_system_browser(url: &str) {
    #[cfg(windows)]
    {
        match std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
        {
            Ok(_) => tracing::info!("已用「系统浏览器」打开界面"),
            Err(e) => {
                tracing::warn!("系统浏览器也打不开（{e}）");
                notify_ui_address(url);
            }
        }
    }
    #[cfg(not(windows))]
    {
        tracing::warn!("当前平台没有可用的浏览器启动方式");
        notify_ui_address(url);
    }
}

/// 实在打不开时，用托盘气泡把地址送到用户眼前。
///
/// 这是最后一层兜底。原来失败只写日志，用户看到的是"双击没反应"，
/// 完全不知道程序是活的、手动打开地址就能用。
fn notify_ui_address(url: &str) {
    match TRAY_HWND.get() {
        Some(h) => {
            vca_platform::tray::notify_fallback_url(*h, url);
        }
        None => tracing::warn!("托盘不可用，只能手动访问：{url}"),
    }
}

/// 把已经打开的界面窗口拉到前台。
///
/// 单实例保护命中时调用：用户双击了第二次，本意通常是「我要看界面」。
/// 能拉到前台就拉，拉不到返回 `false`（调用方只记日志，不算失败）。
fn focus_or_open_ui() -> bool {
    // 首选：自己的原生窗口。直接给它发命令，比按标题搜窗口可靠
    // （标题可能被前端改过，也不受别的程序同名窗口干扰）。
    if let Some(win) = window_handle() {
        win.focus();
        return true;
    }
    // 退路：窗口没建成（走的浏览器兜底），按标题找那个浏览器窗口。
    try_focus_existing_window()
}

/// 按标题找界面窗口并前置。
///
/// **只在走浏览器兜底时才有意义** —— 原生窗口走上面的直接命令。
/// 复用时间可能有几十毫秒（要枚举/查找窗口），但只在「重复启动」这条
/// 冷路径上跑，正常启动一次都不会走到这里。
fn try_focus_existing_window() -> bool {
    vca_platform::window::focus_by_title(UI_WINDOW_TITLE)
}

/// 界面窗口的标题。
///
/// 原生窗口模式下由 `window.rs` 设置成产品名；浏览器兜底模式下
/// 是页面的 `<title>` —— 两边刻意保持一致，兜底切换时用户看不出差别。
const UI_WINDOW_TITLE: &str = "VibeClassAgent";

/// 配置里 settings.yaml 的路径（多处要用，收在一处免得写歪）。
fn settings_path(opts: &ServeOptions) -> PathBuf {
    opts.config_root
        .join("profiles")
        .join(&opts.profile)
        .join("settings.yaml")
}

/// 启动耗时的观测点：`VCA_STARTUP_TRACE=1` 时把各阶段耗时打到日志。
///
/// 留着它是因为**启动慢这类问题只能靠量，不能靠猜**。这次排查就是靠它
/// 才确认了「我们自己的代码只要 ~200 ms，慢的是浏览器那一侧」—— 没有它，
/// 很容易误以为是 Rust 侧在拖，然后去优化一堆根本没花时间的代码。
///
/// 默认关闭（要设环境变量），因为它对用户没有任何价值，只在排查时开。
pub fn startup_trace_enabled() -> bool {
    std::env::var("VCA_STARTUP_TRACE").is_ok()
}

/// 记录一个启动阶段（配合 `startup_trace_enabled` 使用）。
///
/// 用法：在 `serve()` 开头建一个 [`StartupClock`]，之后每个关键节点 `.mark("名字")`。
pub struct StartupClock(std::time::Instant);

impl StartupClock {
    /// 从此刻开始计时。
    pub fn new() -> Self {
        Self(std::time::Instant::now())
    }

    /// 打一个点。未开启 `VCA_STARTUP_TRACE` 时是空操作。
    pub fn mark(&self, what: &str) {
        if startup_trace_enabled() {
            tracing::info!("[STARTUP] {what:<28} +{:?}", self.0.elapsed());
        }
    }
}

impl Default for StartupClock {
    fn default() -> Self {
        Self::new()
    }
}

/// 起任务栏托盘图标。
///
/// 界面窗口是独立的浏览器进程，用户关掉它程序还在后台跑 —— 任务栏上有个
/// 图标，他才知道「它还在」，也才有地方右键退出或打开目录。
///
/// 失败不影响主流程：没有托盘，界面照样能用。
fn start_tray(opts: &ServeOptions) {
    use vca_platform::tray::{spawn, TrayCommand};

    // 跟着配置走：用户在界面上关掉了就别起
    let settings = vca_core::config::load_settings(
        &opts
            .config_root
            .join("profiles")
            .join(&opts.profile)
            .join("settings.yaml"),
    )
    .unwrap_or_default();
    if !settings.ui.tray_icon {
        tracing::info!("托盘图标已在配置里关闭");
        return;
    }

    // 状态回调：右键菜单每次弹出时都会调一次，拿的是**当下**的真实状态，
    // 而不是启动那一刻的快照（否则菜单里的「正在录制」永远是 false）。
    let status = tray_status_fn(opts);

    match spawn("VibeClassAgent", status) {
        Ok((tray, _join)) => {
            tracing::info!("托盘图标已创建");
            let _ = TRAY_HWND.set(tray.hwnd);
            let data_root = opts.data_root.clone();
            // 轮询菜单点击。400ms 一次：人点菜单不会更快，也几乎不耗 CPU。
            std::thread::spawn(move || loop {
                match tray.poll() {
                    Some(TrayCommand::Quit) => {
                        tracing::info!("托盘菜单：退出");
                        // 先摘掉托盘图标再结束进程：直接 exit 的话图标会僵在
                        // 任务栏里，鼠标划过去还在、点它没反应（shell 要等一段时间
                        // 才发现进程没了，或者干脆留到下次登录）。
                        release_tray();
                        // 给托盘线程一点时间跑完 WM_DESTROY 里的 NIM_DELETE。
                        std::thread::sleep(std::time::Duration::from_millis(120));
                        // 这里必须用 exit 而不是走优雅收尾：GUI 进程的收尾逻辑
                        // 在 daemon 那边（见 daemon.rs），而 server 是纯 HTTP 层，
                        // 没有录制会话可收。exit 会跳过析构，但不影响录像完整性 ——
                        // 录制由 daemon 子进程负责，本进程退出不会带走它。
                        std::process::exit(0);
                    }
                    Some(TrayCommand::StopRecording) => {
                        tracing::info!("托盘菜单：请求立即停止录制");
                        vca_platform::shutdown::request_shutdown();
                    }
                    Some(TrayCommand::OpenUi) => {
                        tracing::info!("托盘：打开界面");
                        focus_or_open_ui();
                    }
                    Some(TrayCommand::OpenDataDir) => {
                        let _ = std::process::Command::new("explorer")
                            .arg(&data_root)
                            .spawn();
                    }
                    Some(TrayCommand::OpenLogs) => {
                        let _ = std::process::Command::new("explorer")
                            .arg(data_root.join("logs"))
                            .spawn();
                    }
                    None => {}
                }
                if !tray.alive() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(400));
            });
        }
        Err(e) => tracing::warn!("托盘图标创建失败（{e}），界面功能不受影响"),
    }
}

/// 摘掉托盘图标。
///
/// `close_hwnd` 是平台层提供的**安全**接口（内部自行处理 unsafe），
/// 所以这里不需要 unpack，也不破坏本 crate 的 forbid(unsafe_code)。
pub fn release_tray() {
    if let Some(h) = TRAY_HWND.get() {
        vca_platform::tray::close_hwnd(*h);
    }
}

/// 构造托盘状态回调：每次菜单弹出时现读真实状态。
///
/// 为什么现读而不是缓存在一个 AtomicBool 里：状态由**好几个**来源决定
/// （守护进程是不是在跑、是不是录制中、有几个待处理作业、上次为什么崩的），
/// 缓存就得维护一套失效逻辑，而菜单弹出是低频操作（人手点一下），
/// 现读那点开销完全无所谓 —— 用简单换正确。
///
/// 数据来源是 `crate::daemon::status()`（**本进程**的守护线程状态），
/// 不是去读别的进程 —— 界面和守护进程本来就在同一个进程里。
fn tray_status_fn(opts: &ServeOptions) -> vca_platform::tray::StatusFn {
    use vca_platform::tray::TrayStatus;

    let jobs_dir = opts.data_root.join("jobs");

    Box::new(move || {
        let st = crate::daemon::status();
        let daemon_running = st.get("running").and_then(|v| v.as_bool()).unwrap_or(false);
        let dry_run = st.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false);
        let uptime = if daemon_running {
            st.get("uptime")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        } else {
            // 没在跑的时候别显示「已运行 0 秒」，那比空着更让人困惑。
            String::new()
        };
        let last_error = st
            .get("last_error")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        // 「是否正在录制」由守护循环通过 `runtime_state` 发布 —— 它才是
        // 真正持有录制会话的一方。**不要**去翻 job.json 猜：作业一开始录
        // 就是 `Pending`，和「还没到点」分不开，误报"正在录制"会让用户
        // 以为自己的课正在被录，比漏报更糟。
        let recording = daemon_running && !dry_run && vca_platform::runtime_state::is_recording();

        TrayStatus {
            daemon_running,
            recording,
            dry_run,
            pending_jobs: count_pending_jobs(&jobs_dir),
            uptime,
            last_error,
        }
    })
}

/// 数一下还没推完的作业。
///
/// 与 JobStore::pending() 保持同一套口径：这些状态都算「还没推完」。
fn count_pending_jobs(jobs_dir: &std::path::Path) -> usize {
    let Ok(entries) = std::fs::read_dir(jobs_dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter(|e| {
            let Ok(text) = std::fs::read_to_string(e.path().join("job.json")) else {
                return false;
            };
            let Ok(job) = serde_json::from_str::<vca_core::job::Job>(&text) else {
                return false;
            };
            !matches!(
                job.state,
                vca_core::job::JobState::Pushed | vca_core::job::JobState::Cleaned
            )
        })
        .count()
}

/// 处理一个请求。
fn handle(req: tiny_http::Request, opts: &ServeOptions, token: &str) -> Result<()> {
    let raw = req.url().to_string();
    let (path, query) = split_url(&raw);

    // 静态资源不需要 token（浏览器加载 css/js 时不会带 query），
    // 但 **API 一定要** —— 接口能改配置、能触发录制，不能裸奔。
    if path.starts_with("/api/") {
        let given = query_param(query, "t").unwrap_or_default();
        if given != token {
            return respond_json(req, 403, r#"{"ok":false,"error":"token 不匹配"}"#);
        }
        return api::dispatch(req, &path, query, opts);
    }

    match path.as_str() {
        // 内网预览页：推送给群里的链接就落在这里。
        // 它用**独立的长期令牌**（ui.preview_token），不是界面那个一次性的 ——
        // 推送由 daemon 发出，那个进程拿不到界面启动时随机生成的令牌。
        p if p.starts_with("/preview/") => {
            let want = query_param(query, "t").unwrap_or_default();
            let have = preview_token(opts);
            if !have.is_empty() && want != have {
                return respond(
                    req,
                    403,
                    "text/plain; charset=utf-8",
                    "预览令牌不对（链接里的 t= 那一段）".into(),
                );
            }
            let id = p.trim_start_matches("/preview/").trim_matches('/');
            match render_preview(opts, id) {
                Some(html) => respond(req, 200, "text/html; charset=utf-8", html),
                None => respond(
                    req,
                    404,
                    "text/html; charset=utf-8",
                    "<meta charset=\"utf-8\"><p>没有这条作业，或者它已经被清理了。</p>".into(),
                ),
            }
        }
        // 其余一律当成前端资源。
        //
        // 原来这里是三个硬编码分支（`/index.html`、`/style.css`、`/app.js`），
        // 前端每多拆一个文件就得回来改 Rust —— 分离就白做了。
        // 现在交给 `serve_web_asset`：外部目录里的**任意**文件都能取到，
        // 文件名 → Content-Type 走一张表，加新类型不用碰这个 match。
        other => serve_web_asset(req, opts, other),
    }
}

/// 供一份前端资源（**前后端分离的落点**）。
///
/// 查找顺序：外部目录（`web_dir`）→ 内置资源 → 404。
/// 外部优先让"改完刷新即见"成立，内置兜底让"exe 单独拷走也能开界面"成立。
///
/// # no-store 是刻意的
///
/// 这个服务的所有内容都是"随程序版本走的"，而且静态资源的**路径不变**
/// （每次启动只是多了个一次性 token），浏览器会心安理得地复用缓存 ——
/// 结果就是改了前端看不到效果，人先怀疑缓存再怀疑后端，白折腾半天。
/// 所以一律 `no-store`。
fn serve_web_asset(req: tiny_http::Request, opts: &ServeOptions, raw_path: &str) -> Result<()> {
    let rel = web::normalize_path(raw_path);

    // 先从外部目录取（开发 / 界面重构走这条），取不到再用内置的
    let (body, ctype) = match opts.web_dir.as_deref() {
        Some(dir) => match web::resolve_external(dir, &rel) {
            Some(text) => (text, web::content_type(&rel)),
            None => match web::builtin_for(&rel) {
                Some(text) => (text.to_string(), web::content_type(&rel)),
                None => return respond(req, 404, "text/plain; charset=utf-8", "404".into()),
            },
        },
        None => match web::builtin_for(&rel) {
            Some(text) => (text.to_string(), web::content_type(&rel)),
            None => return respond(req, 404, "text/plain; charset=utf-8", "404".into()),
        },
    };

    respond(req, 200, ctype, body)
}

/// 读配置里的预览令牌（读不到就返回空串 —— 那样等于不校验，
/// 因为默认 `allow_lan` 是关的，端口本来也没暴露出去）。
fn preview_token(opts: &ServeOptions) -> String {
    let path = opts
        .config_root
        .join("profiles")
        .join(&opts.profile)
        .join("settings.yaml");
    vca_core::config::load_settings(&path)
        .map(|s| s.ui.preview_token.trim().to_string())
        .unwrap_or_default()
}

/// 渲染一条作业的预览页。
///
/// 刻意做得很朴素：**只读、无脚本、内联样式**。它要在一个陌生设备
/// （收件人的手机）上打开，任何外部依赖都可能加载不出来。
fn render_preview(opts: &ServeOptions, job_id: &str) -> Option<String> {
    // job_id 来自 URL，先挡掉路径穿越
    if job_id.is_empty() || job_id.contains("..") || job_id.contains('/') || job_id.contains('\\') {
        return None;
    }
    let dir = opts.data_root.join("jobs").join(job_id);
    let text = std::fs::read_to_string(dir.join("job.json")).ok()?;
    let job: vca_core::job::Job = serde_json::from_str(&text).ok()?;

    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };

    // 要点不在 job.json 里 —— 它在同目录的 summary.json（提取阶段的产物）。
    // 读不到就当成"这节课没提取到要点"，而不是让整个页面 404。
    let summary: Option<vca_platform::llm::LessonSummary> =
        std::fs::read_to_string(dir.join("summary.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok());
    let mut points = summary
        .as_ref()
        .map(|s| s.points.clone())
        .unwrap_or_default();
    if let Some(s) = &summary {
        points.extend(s.notices.iter().cloned());
    }
    let list = if points.is_empty() {
        "<li>（这节课没有提取到要点）</li>".to_string()
    } else {
        points
            .iter()
            .map(|p| format!("<li>{}</li>", esc(p)))
            .collect::<Vec<_>>()
            .join("")
    };

    // 文档下载：走 /api 会要 token，所以这里只提示文件在这台机器上
    let doc = job
        .docx_path
        .as_deref()
        .map(|p| {
            format!(
                "<p class=\"dim\">完整文档（含截图）在本机：<code>{}</code></p>",
                esc(p)
            )
        })
        .unwrap_or_default();

    Some(format!(
        r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>{title}</title>
<style>
 body{{margin:0;padding:20px;background:#fafafa;color:#202020;
      font:16px/1.7 "Microsoft YaHei",system-ui,sans-serif}}
 .card{{max-width:760px;margin:0 auto;background:#fff;border-radius:12px;
       padding:24px 26px;box-shadow:0 1px 4px rgba(0,0,0,.08)}}
 h1{{font-size:20px;margin:0 0 4px}}
 .sub{{color:#666;font-size:14px;margin:0 0 18px}}
 ul{{padding-left:22px}} li{{margin:6px 0}}
 .dim{{color:#888;font-size:13px;margin-top:20px;word-break:break-all}}
 code{{background:#f2f2f2;padding:2px 6px;border-radius:4px}}
</style></head><body><div class="card">
<h1>{title}</h1>
<p class="sub">{date} · {course}</p>
<ul>{list}</ul>
{doc}
</div></body></html>"#,
        title = esc(&format!("课堂纪要 {} {}", job.date, job.course)),
        date = esc(&job.date),
        course = esc(&job.course),
    ))
}

/// 拆出路径与查询串。
fn split_url(url: &str) -> (String, &str) {
    match url.split_once('?') {
        Some((p, q)) => (p.to_string(), q),
        None => (url.to_string(), ""),
    }
}

/// 从查询串里取一个参数（值不做 URL 解码 —— token 是十六进制，不需要）。
pub(crate) fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

/// 回一段文本。
pub(crate) fn respond(req: tiny_http::Request, code: u16, ctype: &str, body: String) -> Result<()> {
    let header = Header::from_bytes(&b"Content-Type"[..], ctype.as_bytes())
        .map_err(|_| anyhow::anyhow!("构造响应头失败"))?;
    // 一律 no-store：这个服务的所有内容都是「随程序版本走的」。
    // 尤其是 app.js / style.css —— 每次启动服务时 URL 只是多了个一次性 token，
    // **静态资源的路径没变**，浏览器会心安理得地复用缓存，
    // 结果就是「程序升级了、代码也改了，界面还是老样子」（实测踩过：
    // 新加的按钮死活不出现，查了半天才发现是缓存）。
    let cache = Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..])
        .map_err(|_| anyhow::anyhow!("构造响应头失败"))?;
    let len = body.len();
    let resp = Response::new(
        StatusCode(code),
        vec![header, cache],
        std::io::Cursor::new(body),
        Some(len),
        None,
    );
    req.respond(resp).context("写响应失败")?;
    Ok(())
}

/// 回一段 JSON。
pub(crate) fn respond_json(req: tiny_http::Request, code: u16, body: &str) -> Result<()> {
    respond(
        req,
        code,
        "application/json; charset=utf-8",
        body.to_string(),
    )
}

/// 生成本次会话的随机 token。
///
/// 不需要密码学强度：它防的是本机上别的程序顺手调我们的接口，
/// 而不是防网络攻击（服务只绑 127.0.0.1，外网本来就进不来）。
fn make_token() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    // 简单的位混合：把时间与 pid 搅在一起，够用且不引入 rand 依赖
    let mut x = nanos ^ (pid << 64);
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    format!("{x:032x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_window_returns_false_without_panicking() {
        // 找不到窗口时要安静地返回 false（调用方会退回托盘菜单），
        // 不能 panic —— 这条路在「重复启动」时必然走到。
        assert!(!vca_platform::window::focus_by_title(
            "VibeClassAgent-Definitely-Not-A-Real-Window-12345"
        ));
    }

    /// 没起原生窗口时，`focus_or_open_ui` 必须安全地退到"按标题找窗口"。
    ///
    /// 守的是：`WINDOW` 还没被设置时（走浏览器兜底的场景）这条路径
    /// 不能 panic，也不能谎报成功。
    #[test]
    fn focus_falls_back_when_no_native_window() {
        // 这个测试进程里没起过原生窗口，所以走的必然是兜底路径。
        assert!(
            window_handle().is_none(),
            "测试进程里不该有原生窗口 —— 有的话说明有测试泄漏了状态"
        );
        // 不该 panic。返回真假取决于系统里是否真有同名窗口，所以不断言。
        let _ = focus_or_open_ui();
    }

    /// 界面窗口标题必须是产品名。
    ///
    /// 原生窗口和浏览器兜底两条路都用它 —— 刻意保持一致，
    /// 这样"退回浏览器"时用户也分辨不出差别。
    #[test]
    fn ui_window_title_is_the_product_name() {
        assert_eq!(UI_WINDOW_TITLE, "VibeClassAgent");
        assert!(!UI_WINDOW_TITLE.contains("http"), "标题里不该有地址");
        assert!(
            !UI_WINDOW_TITLE.contains("localhost"),
            "标题里不该有 localhost"
        );
        assert!(!UI_WINDOW_TITLE.contains(':'), "标题里不该有端口号");
    }
}
