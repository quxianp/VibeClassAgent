//! HTTP 服务：只绑回环地址，用系统浏览器以「应用模式」打开界面。

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
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

    // 单实例保护：**必须抢在起托盘/拉浏览器之前**。
    //
    // 没有这道闸时，用户「双击没反应 -> 再双击」每点一次就多一整套后台：
    // 多个托盘图标、多个 Edge 窗口、多个常驻进程，内存线性叠加。
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

    // 预热 Edge 的 profile —— **必须在开窗口之前、且与后面的准备工作并行**。
    //
    // 为什么：整个启动里最慢的一段不是我们自己的代码（实测 222 ms 就绪），
    // 而是 Edge 起来后要建/校验用户数据目录。那个目录第一次不存在时，
    // Edge 要解压资源、建 SQLite、跑首次运行检查，冷启动能到好几秒。
    //
    // 这里先空跑一次 Edge 把目录建出来，等真正 `--app=` 打开时它已经是热的。
    // 但不能串行地等它 —— 那等于把慢的那段挪到前面，总时长没变。
    // 所以丢到后台线程，和「起托盘 / 生成预览令牌」这些活并行。
    if opts.open_browser {
        warm_up_profile();
    }

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
    if opts.open_browser {
        match open_app_window(&url) {
            Ok(how) => tracing::info!("已用「{how}」打开界面"),
            Err(e) => {
                // 打不开浏览器不该让程序退出：地址已经打出来了，用户手动点开也能用。
                tracing::warn!("自动打开浏览器失败（{e}），请手动访问上面的地址");
            }
        }
        clock.mark("已拉起浏览器（之后的耗时都在浏览器侧）");
    }

    for req in server.incoming_requests() {
        if let Err(e) = handle(req, &opts, &token) {
            tracing::warn!("处理请求出错：{e}");
        }
    }
    Ok(())
}

/// 把已经打开的界面窗口拉到前台。
///
/// 单实例保护命中时调用：用户双击了第二次，本意通常是「我要看界面」。
/// 能拉到前台就拉，拉不到返回 `false`（调用方只记日志，不算失败）。
fn focus_or_open_ui() -> bool {
    try_focus_existing_window()
}

/// 按标题找界面窗口并前置。
///
/// 复用时间可能有几十毫秒（要枚举/查找窗口），但只在「重复启动」这条
/// 冷路径上跑，正常启动一次都不会走到这里。
fn try_focus_existing_window() -> bool {
    vca_platform::window::focus_by_title(UI_WINDOW_TITLE)
}

/// 界面窗口的标题（Edge `--app=` 模式下就是页面 `<title>`）。
const UI_WINDOW_TITLE: &str = "VibeClassAgent";

/// 配置里 settings.yaml 的路径（多处要用，收在一处免得写歪）。
fn settings_path(opts: &ServeOptions) -> PathBuf {
    opts.config_root
        .join("profiles")
        .join(&opts.profile)
        .join("settings.yaml")
}

/// 在后台把 Edge 的 profile 预热出来。
///
/// # 为什么需要这个
///
/// 启动慢的**不是我们自己的代码**：实测从进程起来到 HTTP 服务能响应只要
/// 约 900 ms，其中日志显示「界面地址」到「已用 Edge 应用模式打开界面」
/// 之间只有约 160 ms。真正让用户觉得"等很久"的是 **Edge 建/校验
/// 用户数据目录**这一步 —— 那个目录不存在时，Edge 要解压资源、建 SQLite
/// 数据库、跑一圈首次运行检查，冷启动能到好几秒。
///
/// # 为什么不直接等它
///
/// 串行地"先预热再开窗口"等于把慢的那段原样挪到前面，用户等待的总时长
/// 一点没变，只是慢的动作换了个位置。所以这里**丢到后台线程**，
/// 与「起托盘」「生成预览令牌」这些活并行跑；等真正要用时它多半已经好了。
///
/// # 为什么是空跑一个 `about:blank`
///
/// 只是为了让 Edge 把目录结构和数据库建出来，不需要它真渲染我们的页面 ——
/// 所以开一个最小页面，并且加 `--no-startup-window` 之外不加任何多余的旗标。
/// 起来之后立刻关掉：用户不应该看到第二个窗口闪一下。
///
/// 失败一律吞掉：预热只是优化，失败了无非是回到"第一次打开慢一点"，
/// 绝不能因此让程序报错或退出。
fn warm_up_profile() {
    let profile = vca_temp_dir();
    // 已经预热过（目录里有 Edge 的标记文件）就不用再跑一趟，
    // 免得每次启动都白起一个进程。
    if profile.join("Default").join("Preferences").is_file() {
        return;
    }

    std::thread::Builder::new()
        .name("vca-profile-warmup".into())
        .spawn(move || {
            let Some(exe) = edge_candidates().into_iter().find(|c| c.is_file()) else {
                return;
            };
            let started = std::time::Instant::now();
            // 用 `--headless` 起一个最小实例：不画窗口、不进任务栏，
            // 但照样会把 profile 目录建出来 —— 这正是我们要的副作用。
            //
            // 注意这里**不能**带 `--user-data-dir` 以外的业务参数，
            // 尤其是不能带 `--app=`：那会真的开出一个窗口。
            let child = Command::new(exe)
                .args([
                    "--headless",
                    "--disable-gpu",
                    "--no-first-run",
                    "--no-default-browser-check",
                    // 预热进程也别去后台联网（和正式启动保持一致的克制）
                    "--disable-background-networking",
                    "--disable-features=msEdgeBackgroundPreload,Translate,msEdgeTranslate",
                ])
                .arg(format!("--user-data-dir={}", profile.display()))
                .arg("about:blank")
                .spawn();

            match child {
                Ok(mut c) => {
                    // 等它把 profile 写完。给一个上限，避免 Edge 卡住时
                    // 这个后台线程一直挂着（虽然不影响主流程，但没必要）。
                    let deadline = std::time::Duration::from_secs(20);
                    loop {
                        match c.try_wait() {
                            Ok(Some(_)) => break,
                            Ok(None) if started.elapsed() < deadline => {
                                std::thread::sleep(std::time::Duration::from_millis(100));
                            }
                            _ => {
                                // 超时或出错：收掉子进程，别再挂着
                                let _ = c.kill();
                                break;
                            }
                        }
                    }
                    tracing::debug!("profile 预热结束（{:?}）", started.elapsed());
                }
                Err(e) => tracing::debug!("profile 预热跳过（{e}）"),
            }
        })
        .ok();
}

/// 启动耗时的观测点：`VCA_STARTUP_TRACE=1` 时把各阶段耗时打到日志。
///
/// 留着它是因为**启动慢这类问题只能靠量，不能靠猜**。这次排查就是靠它
/// 才确认了「我们自己的代码只要 ~200 ms，慢的是 Edge」—— 没有这个观测点，
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
        "/" | "/index.html" => {
            let body = web::resolve(opts.web_dir.as_deref(), "index.html", web::INDEX_HTML);
            respond(req, 200, "text/html; charset=utf-8", body)
        }
        "/style.css" => {
            let body = web::resolve(opts.web_dir.as_deref(), "style.css", web::STYLE_CSS);
            respond(req, 200, "text/css; charset=utf-8", body)
        }
        "/app.js" => {
            let body = web::resolve(opts.web_dir.as_deref(), "app.js", web::APP_JS);
            respond(req, 200, "application/javascript; charset=utf-8", body)
        }
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
        _ => respond(req, 404, "text/plain; charset=utf-8", "404".into()),
    }
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

/// 以「应用模式」打开窗口：没有地址栏、独立窗口、有任务栏图标。
///
/// 优先 Edge（Win10 一定自带），找不到就退回默认浏览器（会有地址栏，但能用）。
fn open_app_window(url: &str) -> Result<&'static str> {
    for cand in edge_candidates() {
        if cand.is_file() {
            Command::new(cand)
                .args(edge_args(url, &vca_temp_dir()))
                .spawn()
                .context("启动 Edge 失败")?;
            return Ok("Edge 应用模式");
        }
    }
    // 回退：交给系统默认浏览器（会有地址栏）
    #[cfg(windows)]
    {
        Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .context("打开默认浏览器失败")?;
        Ok("默认浏览器")
    }
    #[cfg(not(windows))]
    {
        anyhow::bail!("当前平台没有可用的浏览器启动方式")
    }
}

/// 拼 Edge 的启动参数。
///
/// 单独抽出来是为了**可测**：这些标志多数是「反直觉但必需」的，
/// 而它们的效果只有肉眼能看出来（窗口有没有地址栏、按 F12 有没有反应），
/// 写成测试至少能防住「重构时手一抖删掉一个」。
///
/// 每个标志为什么在这里：
/// - `--app=`：应用模式，去掉地址栏/标签栏，这是"看起来像原生程序"的关键。
/// - `--user-data-dir=`：独立 profile。**必须**用独立的，否则会和用户自己开的
///   Edge 抢同一个 profile，`--app` 会被当成"在当前窗口开个标签"，就没有独立窗口了。
/// - `--no-first-run` / `--no-default-browser-check`：跳过首次运行的欢迎向导
///   和默认浏览器询问。**这两个是冷启动慢的主因之一** —— 没有它们时，
///   即使 profile 已存在，Edge 仍会做一轮首次运行检查。
/// - `--disable-background-networking`：别在后台偷偷联网同步/更新。
/// - `--disable-features=...`：**只能出现一次**。DevTools 关掉开发者工具，
///   Translate / msEdgeTranslate 关掉翻译气泡（页面上弹一条"是否翻译"很出戏），
///   msEdgeBackgroundPreload 关掉后台预加载。
///   ⚠️ 写两处 `--disable-features=` 不会合并，**后者会静默覆盖前者**。
/// - `--disable-translate` / `--disable-save-password-bubble`：翻译与密码气泡，
///   都是"这看起来是个浏览器"的破绽。
/// - `--no-service-autorun` / `--disable-component-update`：别装后台服务、
///   别自动更新组件（一体机上我们不想让 Edge 在后台折腾）。
fn edge_args(url: &str, profile_dir: &std::path::Path) -> Vec<String> {
    vec![
        format!("--app={url}"),
        format!("--user-data-dir={}", profile_dir.display()),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--disable-background-networking".into(),
        "--disable-features=msEdgeBackgroundPreload,DevTools,Translate,msEdgeTranslate".into(),
        "--disable-translate".into(),
        "--disable-save-password-bubble".into(),
        "--no-service-autorun".into(),
        "--disable-component-update".into(),
    ]
}

/// 可能的 Edge 可执行文件位置。
fn edge_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    for env in ["ProgramFiles(x86)", "ProgramFiles", "ProgramW6432"] {
        if let Ok(base) = std::env::var(env) {
            v.push(
                PathBuf::from(base)
                    .join("Microsoft")
                    .join("Edge")
                    .join("Application")
                    .join("msedge.exe"),
            );
        }
    }
    v
}

/// 给界面窗口用的独立用户数据目录。
///
/// **必须放在一个稳定的位置**，不能放 `%TEMP%`。原因：
/// `%TEMP%` 会被系统/清理软件定期清空，一旦 profile 没了，Edge 下次启动
/// 就得从零建 profile（解压、建数据库、首次运行检查），冷启动肉眼可见地慢。
/// 更糟的是这个目录每次被清掉，用户就要再等一次。
///
/// 所以放 `%LOCALAPPDATA%\VibeClassAgent\ui-profile` —— 同样是"用户私有、
/// 不进版本库"的位置，但不会被当垃圾清掉，第二次启动开始就是热的。
fn vca_temp_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("TEMP").map(PathBuf::from))
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("VibeClassAgent").join("ui-profile")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个参数各占一个 argv 位置 —— 不能把 `--user-data-dir=` 和它的值
    /// 拼成一个字符串（Edge 会把它当成一个未知参数，profile 路径就丢了）。
    fn args_for(url: &str) -> Vec<String> {
        edge_args(url, std::path::Path::new(r"C:\tmp\profile"))
    }

    #[test]
    fn app_mode_flag_present() {
        let a = args_for("http://127.0.0.1:1234/?t=abc");
        assert!(
            a.iter().any(|x| x.starts_with("--app=")),
            "缺少 --app=，窗口会有地址栏（看起来就是个浏览器）"
        );
    }

    #[test]
    fn url_goes_into_app_flag_verbatim() {
        let url = "http://127.0.0.1:45678/?t=deadbeef";
        let a = args_for(url);
        assert!(a.contains(&format!("--app={url}")));
    }

    #[test]
    fn user_data_dir_follows_dash_form() {
        let a = args_for("http://127.0.0.1:1/?t=x");
        assert!(
            a.iter().any(|x| x.starts_with("--user-data-dir=")),
            "缺少独立 profile，会和用户自己的 Edge 抢 profile，--app 会退化成开标签"
        );
    }

    /// 冷启动慢的主因：少了这两个标志，Edge 每次都要跑一轮首次运行检查。
    #[test]
    fn cold_start_flags_present() {
        let a = args_for("http://127.0.0.1:1/?t=x");
        assert!(a.iter().any(|x| x == "--no-first-run"));
        assert!(a.iter().any(|x| x == "--no-default-browser-check"));
    }

    #[test]
    fn devtools_are_disabled() {
        let a = args_for("http://127.0.0.1:1/?t=x");
        let feats: Vec<&String> = a
            .iter()
            .filter(|x| x.starts_with("--disable-features="))
            .collect();
        // **只能有一条**：写两处不会合并，后者会静默覆盖前者。
        assert_eq!(feats.len(), 1, "--disable-features 只能出现一次");
        assert!(feats[0].contains("DevTools"), "开发者工具没关掉");
    }

    #[test]
    fn preload_disabled_alongside_devtools() {
        let a = args_for("http://127.0.0.1:1/?t=x");
        let feats: Vec<&String> = a
            .iter()
            .filter(|x| x.starts_with("--disable-features="))
            .collect();
        assert!(feats[0].contains("msEdgeBackgroundPreload"));
    }

    #[test]
    fn translate_bubble_disabled() {
        let a = args_for("http://127.0.0.1:1/?t=x");
        assert!(
            a.iter().any(|x| x == "--disable-translate"),
            "翻译气泡会冒出来，很像浏览器"
        );
        let feats: Vec<&String> = a
            .iter()
            .filter(|x| x.starts_with("--disable-features="))
            .collect();
        assert!(feats[0].contains("Translate"));
    }

    /// profile 必须落在**稳定**位置。放 `%TEMP%` 会被清理软件删掉，
    /// 于是每次冷启动都要重建 profile —— 这正是启动慢的元凶。
    #[test]
    fn profile_path_is_stable_and_not_in_temp_root() {
        let dir = vca_temp_dir();
        let s = dir.to_string_lossy().to_lowercase();

        assert!(
            s.contains("vibeclassagent"),
            "profile 路径里没有程序名，太容易被误删：{s}"
        );
        assert!(s.ends_with("ui-profile"));

        // 不能是 %TEMP% 的直接子目录
        if let Ok(temp) = std::env::var("TEMP") {
            let t = PathBuf::from(temp);
            assert_ne!(
                dir.parent(),
                Some(t.as_path()),
                "profile 直接躺在 %TEMP% 下，会被清掉"
            );
        }
    }

    #[test]
    fn profile_dir_is_absolute() {
        // 相对路径会随工作目录漂移：从开始菜单启动和从命令行启动会得到
        // 两个不同的 profile，每个都要重建一遍。
        assert!(vca_temp_dir().is_absolute() || std::env::var("LOCALAPPDATA").is_err());
    }

    /// 预热是「优化」不是「必需」：不论 Edge 在不在、能不能起，
    /// 这个函数都必须立刻返回、绝不 panic、绝不阻塞调用方。
    ///
    /// 这条测试同时守着一个容易犯的错：有人后来把它改成同步等待，
    /// 那就会把启动时长直接拖长，而"测试还是绿的"（因为功能没坏）。
    #[test]
    fn warm_up_never_blocks_or_panics() {
        let t0 = std::time::Instant::now();
        warm_up_profile();
        let took = t0.elapsed();
        assert!(
            took < std::time::Duration::from_millis(1500),
            "warm_up_profile 阻塞了 {took:?} —— 它必须是后端异步的"
        );
    }

    #[test]
    fn missing_window_returns_false_without_panicking() {
        // 找不到窗口时要安静地返回 false（调用方会退回托盘菜单），
        // 不能 panic —— 这条路在「重复启动」时必然走到。
        assert!(!vca_platform::window::focus_by_title(
            "VibeClassAgent-Definitely-Not-A-Real-Window-12345"
        ));
    }
}
