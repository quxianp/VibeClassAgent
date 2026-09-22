//! HTTP 服务：只绑回环地址，用系统浏览器以「应用模式」打开界面。

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};
use tiny_http::{Header, Response, Server, StatusCode};

use crate::api;
use crate::web;

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
    let listener = TcpListener::bind(("127.0.0.1", opts.port))
        .with_context(|| format!("绑定回环端口 {} 失败", opts.port))?;
    let port = listener.local_addr()?.port();
    let server = Server::from_listener(listener, None)
        .map_err(|e| anyhow::anyhow!("创建 HTTP 服务失败：{e}"))?;

    let token = make_token();
    let url = format!("http://127.0.0.1:{port}/?t={token}");

    start_tray(&opts);

    tracing::info!("界面地址：{url}");
    if opts.open_browser {
        match open_app_window(&url) {
            Ok(how) => tracing::info!("已用「{how}」打开界面"),
            Err(e) => {
                // 打不开浏览器不该让程序退出：地址已经打出来了，用户手动点开也能用。
                tracing::warn!("自动打开浏览器失败（{e}），请手动访问上面的地址");
            }
        }
    }

    for req in server.incoming_requests() {
        if let Err(e) = handle(req, &opts, &token) {
            tracing::warn!("处理请求出错：{e}");
        }
    }
    Ok(())
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

    match spawn("VibeClassAgent") {
        Ok((tray, _handle)) => {
            tracing::info!("托盘图标已创建");
            let data_root = opts.data_root.clone();
            // 轮询菜单点击。400ms 一次：人点菜单不会更快，也几乎不耗 CPU。
            std::thread::spawn(move || loop {
                match tray.poll() {
                    Some(TrayCommand::Quit) => {
                        tracing::info!("托盘菜单：退出");
                        std::process::exit(0);
                    }
                    Some(TrayCommand::StopRecording) => {
                        tracing::info!("托盘菜单：停止录制");
                        vca_platform::shutdown::request_shutdown();
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
                .arg(format!("--app={url}"))
                // 用独立用户数据目录：避免和用户自己开的 Edge 抢同一个 profile
                // （抢了的话 --app 会被当成"在当前窗口开个标签"，就没有独立窗口了）
                .arg("--user-data-dir=")
                .arg(vca_temp_dir())
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
fn vca_temp_dir() -> PathBuf {
    let base = std::env::var("TEMP")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("VibeClassAgent").join("ui-profile")
}
