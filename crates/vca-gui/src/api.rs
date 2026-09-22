//! 界面用的后端接口。
//!
//! 所有写操作都走 `vca_core::setup::patch_settings_line`（按 `a.b.c` 逐级定位），
//! **不要自己拼 YAML** —— 那正是上一轮「配好的模型重启就没了」的根因所在。
//! 一句话：这里只做参数校验和格式转换，落盘的事交给那一个函数。

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{json, Value};

use crate::server::ServeOptions;

/// 按路径分发。
pub fn dispatch(
    mut req: tiny_http::Request,
    path: &str,
    _query: &str,
    opts: &ServeOptions,
) -> Result<()> {
    let method = req.method().as_str().to_uppercase();
    let mut body = String::new();
    if method == "POST" {
        // dyn Read 自带 read_to_string（对象安全方法），不需要额外 use
        let _ = req.as_reader().read_to_string(&mut body);
    }

    let settings_path = settings_path(opts);

    let result = match (method.as_str(), path) {
        ("GET", "/api/status") => status(opts, &settings_path),
        ("GET", "/api/providers") => providers(),
        ("GET", "/api/config/llm") => get_llm(&settings_path),
        ("POST", "/api/config/llm") => post_llm(&settings_path, &body),
        ("GET", "/api/config/push") => get_push(&settings_path),
        ("POST", "/api/config/push") => post_push(&settings_path, &body),
        ("POST", "/api/push/test") => push_test(&settings_path, &body),
        ("POST", "/api/push/discover") => push_discover(&body),
        ("GET", "/api/push/discover") => Ok(discover_json()),
        ("GET", "/api/config/general") => get_general(&settings_path),
        ("POST", "/api/config/general") => post_general(&settings_path, &body),
        ("GET", "/api/schedule") => get_schedule(opts),
        ("POST", "/api/schedule") => post_schedule(opts, &body),
        ("GET", "/api/timetable") => get_timetable(opts),
        ("POST", "/api/timetable") => post_timetable(opts, &body),
        ("GET", "/api/jobs") => get_jobs(opts),
        ("POST", "/api/jobs/run") => job_run(opts, &body),
        ("GET", "/api/jobs/run") => Ok(job_run_json()),
        ("POST", "/api/models") => list_models(&settings_path, &body),
        ("POST", "/api/clean/preview") => clean_preview(opts),
        ("GET", "/api/daemon/status") => Ok(crate::daemon::status()),
        ("POST", "/api/daemon/start") => daemon_start(opts, &body),
        ("POST", "/api/daemon/stop") => crate::daemon::stop(),
        ("GET", "/api/logs") => read_logs(opts),
        ("GET", "/api/i18n") => i18n(),
        ("POST", "/api/detect-bot") => detect_bot(),
        ("GET", "/api/bot/napcat") => bot_status(&settings_path),
        ("POST", "/api/bot/napcat/install") => bot_install(&body),
        ("POST", "/api/bot/napcat/start") => bot_start(),
        ("POST", "/api/bot/napcat/stop") => bot_stop(),
        ("POST", "/api/bot/napcat/open") => bot_open_dir(),
        ("POST", "/api/import/classisland") => import_classisland(opts, &body),
        ("POST", "/api/quit") => quit(),
        _ => Ok(json!({"ok": false, "error": format!("没有这个接口：{method} {path}")})),
    };

    match result {
        Ok(v) => crate::server::respond_json(req, 200, &v.to_string()),
        Err(e) => crate::server::respond_json(
            req,
            200,
            &json!({"ok": false, "error": e.to_string()}).to_string(),
        ),
    }
}

/// settings.yaml 的位置。
fn settings_path(opts: &ServeOptions) -> PathBuf {
    opts.config_root
        .join("profiles")
        .join(&opts.profile)
        .join("settings.yaml")
}

/// 确保配置存在（首次打开界面时不该是一片空白）。
fn ensure_settings(opts: &ServeOptions, path: &Path) {
    if !path.exists() {
        let _ = vca_core::setup::repair(
            &vca_core::paths::Layout::new(opts.data_root.clone(), opts.config_root.clone()),
            &opts.profile,
        );
    }
}

/// 写一个配置项。返回是否真的写进去了。
fn set(path: &Path, key: &str, value: &str) -> bool {
    vca_core::setup::patch_settings_line(path, key, value).unwrap_or(false)
}

/// 把字符串写成 YAML 标量（带引号，避免 `a:b` 这种值把文件写坏）。
fn yaml_str(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

// ---------------------------------------------------------------- 状态

fn status(opts: &ServeOptions, settings: &Path) -> Result<Value> {
    ensure_settings(opts, settings);
    let s = vca_core::config::load_settings(settings).unwrap_or_default();
    let layout = vca_core::paths::Layout::new(opts.data_root.clone(), opts.config_root.clone());

    // 本地转写是否就绪
    let whisper = layout
        .data_root
        .parent()
        .map(|_| ())
        .and_then(|_| {
            // tools/whisper/whisper-cli.exe —— 以 exe 所在目录为基准
            let exe = std::env::current_exe().ok()?;
            let root = exe.parent()?;
            Some(root.join("tools").join("whisper").join("whisper-cli.exe"))
        })
        .map(|p| p.is_file())
        .unwrap_or(false);

    let jobs = vca_core::store::JobStore::new(layout.profile_data_dir(&opts.profile));
    let pending = jobs.pending().len();
    let all = jobs.list().len();

    let llm_ready = !s.llm.base_url.trim().is_empty()
        && !s.llm.model.trim().is_empty()
        && !vca_platform::llm::is_placeholder(&s.llm.base_url)
        && !vca_platform::llm::is_placeholder(&s.llm.model);

    Ok(json!({
        "ok": true,
        "profile": opts.profile,
        "config_root": opts.config_root.display().to_string(),
        "data_root": opts.data_root.display().to_string(),
        "llm": {
            "provider": s.llm.provider,
            "base_url": s.llm.base_url,
            "model": s.llm.model,
            "key_set": vca_core::secrets::is_set("VCA_LLM_API_KEY"),
            "ready": llm_ready,
        },
        "push": {
            "provider": s.push.provider,
            "target": s.push.target.clone().unwrap_or_default(),
            "target_type": s.push.target_type,
            "ready": !s.push.provider.trim().is_empty(),
        },
        "whisper_ready": whisper,
        "jobs": { "total": all, "pending": pending },
    }))
}

// ---------------------------------------------------------------- 服务商预设

fn providers() -> Result<Value> {
    let list: Vec<Value> = vca_platform::llm::PROVIDERS
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "base_url": p.base_url,
                "note": p.note,
            })
        })
        .collect();
    Ok(json!({ "ok": true, "providers": list }))
}

// ---------------------------------------------------------------- 模型配置

fn get_llm(path: &Path) -> Result<Value> {
    let s = vca_core::config::load_settings(path).unwrap_or_default();
    Ok(json!({
        "ok": true,
        "provider": s.llm.provider,
        "base_url": s.llm.base_url,
        "model": s.llm.model,
        "mode": s.llm.mode,
        // 只回"有没有"，**不回密钥本身** —— 界面不需要看到它
        "key_set": vca_core::secrets::is_set("VCA_LLM_API_KEY"),
        "defer_on_peak": s.llm.defer_on_peak,
        "peak_holidays": s.llm.peak_holidays,
    }))
}

fn post_llm(path: &Path, body: &str) -> Result<Value> {
    let v: Value = serde_json::from_str(body)?;
    let mut wrote = Vec::new();

    if let Some(p) = v.get("provider").and_then(|x| x.as_str()) {
        if set(path, "llm.provider", &yaml_str(p)) {
            wrote.push("provider");
        }
    }
    if let Some(b) = v.get("base_url").and_then(|x| x.as_str()) {
        if set(path, "llm.base_url", &yaml_str(b)) {
            wrote.push("base_url");
        }
    }
    if let Some(m) = v.get("model").and_then(|x| x.as_str()) {
        if set(path, "llm.model", &yaml_str(m)) {
            wrote.push("model");
        }
    }
    if let Some(d) = v.get("defer_on_peak").and_then(|x| x.as_bool()) {
        if set(path, "llm.defer_on_peak", if d { "true" } else { "false" }) {
            wrote.push("defer_on_peak");
        }
    }
    // 密钥单独存 secrets.env，绝不进 settings.yaml
    if let Some(k) = v.get("api_key").and_then(|x| x.as_str()) {
        if !k.trim().is_empty() {
            let sp = vca_core::secrets::default_path(
                path.parent()
                    .and_then(|p| p.parent())
                    .and_then(|p| p.parent())
                    .unwrap_or(Path::new(".")),
            );
            vca_core::secrets::upsert(&sp, "VCA_LLM_API_KEY", k.trim())?;
            // 立刻注入，这样紧接着拉模型列表就能用
            vca_core::secrets::set_override("VCA_LLM_API_KEY", k.trim());
            wrote.push("api_key");
        }
    }

    // 回读一遍确认真的落盘了（这曾经是个静默失败的地方）
    let after = vca_core::config::load_settings(path).unwrap_or_default();
    Ok(json!({
        "ok": !wrote.is_empty(),
        "wrote": wrote,
        "current": {
            "provider": after.llm.provider,
            "base_url": after.llm.base_url,
            "model": after.llm.model,
        }
    }))
}

// ---------------------------------------------------------------- 推送配置

fn get_push(path: &Path) -> Result<Value> {
    let s = vca_core::config::load_settings(path).unwrap_or_default();
    // 各渠道各记一份「地址 + 目标」：settings.yaml 里那对字段是所有渠道共用的，
    // 切渠道时会看到别人的值（用户就是这么以为「配置没保存」的）。
    let store = vca_core::push_profiles::PushProfiles::load(&profiles_path(path));
    let entry = store.get(&s.push.provider);
    Ok(json!({
        "ok": true,
        "provider": s.push.provider,
        "endpoint": s.push.endpoint,
        "target": s.push.target.clone().unwrap_or_default(),
        "target_type": s.push.target_type,
        "max_retries": s.push.max_retries,
        // 当前渠道记住的那份（界面切回这个渠道时用它预填）
        "current": entry,
        // 全部渠道的记忆，供前端切换时取用
        "profiles": store.profiles,
        // 同样只回"设没设"，不回内容
        "token_set": vca_core::secrets::is_set("VCA_PUSH_TOKEN"),
        "qq_appid_set": vca_core::secrets::is_set("VCA_QQ_APP_ID"),
        "qq_secret_set": vca_core::secrets::is_set("VCA_QQ_APP_SECRET"),
        // 智能机器人的两条凭据：同样只回「设没设」，不回内容
        "wecom_bot_set": vca_core::secrets::is_set("VCA_WECOM_BOT_ID"),
        "wecom_secret_set": vca_core::secrets::is_set("VCA_WECOM_BOT_SECRET"),
    }))
}

/// 渠道记忆文件的位置：配置根目录（`<config>/push-profiles.yaml`）。
fn profiles_path(settings_path: &Path) -> PathBuf {
    settings_path
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .map(vca_core::push_profiles::default_path)
        .unwrap_or_else(|| PathBuf::from("push-profiles.yaml"))
}

fn post_push(path: &Path, body: &str) -> Result<Value> {
    let v: Value = serde_json::from_str(body)?;
    let mut wrote = Vec::new();

    let provider = v
        .get("provider")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();

    if let Some(p) = v.get("provider").and_then(|x| x.as_str()) {
        if set(path, "push.provider", &yaml_str(p)) {
            wrote.push("provider");
        }
    }
    if let Some(e) = v.get("endpoint").and_then(|x| x.as_str()) {
        if set(path, "push.endpoint", &yaml_str(e)) {
            wrote.push("endpoint");
        }
    }
    if let Some(t) = v.get("target").and_then(|x| x.as_str()) {
        if set(path, "push.target", &yaml_str(t)) {
            wrote.push("target");
        }
    }
    if let Some(tt) = v.get("target_type").and_then(|x| x.as_str()) {
        if set(path, "push.target_type", &yaml_str(tt)) {
            wrote.push("target_type");
        }
    }

    // 顺手把这一次的地址与目标记到该渠道名下。
    // 只记请求里**确实带了**的字段，没带的沿用上次记住的值 ——
    // 否则「只改目标类型」的一次保存会把地址抹掉。
    if !provider.trim().is_empty() {
        let f = profiles_path(path);
        let mut store = vca_core::push_profiles::PushProfiles::load(&f);
        let mut entry = store.get(&provider);
        if let Some(e) = v.get("endpoint").and_then(|x| x.as_str()) {
            entry.endpoint = e.to_string();
        }
        if let Some(t) = v.get("target").and_then(|x| x.as_str()) {
            entry.target = t.to_string();
        }
        if let Some(tt) = v.get("target_type").and_then(|x| x.as_str()) {
            entry.target_type = tt.to_string();
        }
        store.remember(&provider, entry);
        if let Err(e) = store.save(&f) {
            tracing::warn!("渠道记忆写不进去（不影响本次保存）：{e}");
        }
    }

    // 凭据进 secrets.env
    let secret_dir = path
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .unwrap_or(Path::new("."));
    let sp = vca_core::secrets::default_path(secret_dir);
    for (field, env) in [
        ("token", "VCA_PUSH_TOKEN"),
        ("qq_app_id", "VCA_QQ_APP_ID"),
        ("qq_app_secret", "VCA_QQ_APP_SECRET"),
        ("wecom_bot_id", "VCA_WECOM_BOT_ID"),
        ("wecom_bot_secret", "VCA_WECOM_BOT_SECRET"),
    ] {
        if let Some(val) = v.get(field).and_then(|x| x.as_str()) {
            if !val.trim().is_empty() {
                vca_core::secrets::upsert(&sp, env, val.trim())?;
                vca_core::secrets::set_override(env, val.trim());
                wrote.push(field);
            }
        }
    }

    let after = vca_core::config::load_settings(path).unwrap_or_default();
    Ok(json!({
        "ok": !wrote.is_empty(),
        "wrote": wrote,
        "current": {
            "provider": after.push.provider,
            "endpoint": after.push.endpoint,
            "target": after.push.target.clone().unwrap_or_default(),
            "target_type": after.push.target_type,
        }
    }))
}

/// 会话发现的进度。
///
/// 为什么要有这么个状态：企业微信智能机器人拉会话要**连上 WS 监听十几秒**
/// （它是回调制，会话 id 只会在别人说话时送过来）。而本服务是单线程的，
/// 同步在那儿等十几秒会把整个界面卡死 —— 连「好了没」都问不出来。
/// 所以挪到后台线程，前端轮询这个状态。
struct DiscoverState {
    running: bool,
    chats: Vec<(String, String)>,
    /// 认不出的帧样例（官方改字段名时用来定位）。
    unknown: Vec<String>,
    error: Option<String>,
    note: String,
}

static DISCOVER: std::sync::Mutex<DiscoverState> = std::sync::Mutex::new(DiscoverState {
    running: false,
    chats: Vec::new(),
    unknown: Vec::new(),
    error: None,
    note: String::new(),
});

fn discover_state() -> std::sync::MutexGuard<'static, DiscoverState> {
    DISCOVER.lock().unwrap_or_else(|e| e.into_inner())
}

fn discover_json() -> Value {
    let st = discover_state();
    json!({
        "ok": true,
        "running": st.running,
        "chats": st.chats
            .iter()
            .map(|(id, name)| json!({"id": id, "name": name}))
            .collect::<Vec<_>>(),
        "unknown_frames": st.unknown,
        "error": st.error,
        "note": st.note,
    })
}

/// 取一条凭据：请求体里的优先，其次 secrets.env。
fn pick(v: &Value, key: &str, env: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(String::from)
        .unwrap_or_else(|| vca_core::secrets::get(env))
}

/// 开始找会话。
///
/// 两个渠道的取法完全不同：
/// - **Telegram**：`getUpdates` 一次调用就列出来（用户得先给机器人发过话）。
/// - **企业微信智能机器人**：企业微信**不提供**会话列表接口，只能连上 WS
///   监听一段时间，把期间出现过的会话收集起来 —— 所以这里得等十几秒。
///
/// 两种都放到后台线程里跑，立刻返回；前端轮询 [`discover_json`]。
fn push_discover(body: &str) -> Result<Value> {
    if discover_state().running {
        return Ok(json!({"ok": false, "error": "上一次还在找，等它跑完"}));
    }

    let v: Value = serde_json::from_str(body).unwrap_or_else(|_| json!({}));
    let provider = v
        .get("provider")
        .and_then(|x| x.as_str())
        .unwrap_or("telegram")
        .to_string();

    {
        let mut st = discover_state();
        st.running = true;
        st.chats.clear();
        st.unknown.clear();
        st.error = None;
        st.note = String::new();
    }

    let listen = v.get("seconds").and_then(|x| x.as_u64()).unwrap_or(15);

    if provider == "wecom-aibot" {
        let bot_id = pick(&v, "wecom_bot_id", "VCA_WECOM_BOT_ID");
        let secret = pick(&v, "wecom_bot_secret", "VCA_WECOM_BOT_SECRET");
        std::thread::spawn(move || {
            let cfg = vca_platform::wecom_aibot::AibotConfig::new(&bot_id, &secret, "", "");
            let mut st = discover_state();
            match vca_platform::wecom_aibot::list_chats(&cfg, listen) {
                Ok(found) => {
                    st.chats = found.chats;
                    st.unknown = found.unknown_frames;
                    st.note = "企业微信没有「列出全部会话」的接口（这是它的隐私设计），\
                               所以这里列的是**最近和机器人有过互动的会话**。\
                               想把某个群加进来，先把机器人拉进群、在群里 @ 它一次，再来点一次。"
                        .to_string();
                }
                Err(e) => st.error = Some(e.to_string()),
            }
            st.running = false;
        });
        return Ok(json!({"ok": true, "message": format!("正在监听 {listen} 秒…")}));
    }

    // Telegram
    let token = pick(&v, "token", "VCA_PUSH_TOKEN");
    std::thread::spawn(move || {
        let mut st = discover_state();
        match vca_platform::bots::telegram_chat_ids(&token, 20_000) {
            Ok(list) => st.chats = list,
            Err(e) => st.error = Some(e.to_string()),
        }
        st.running = false;
    });
    Ok(json!({"ok": true, "message": "正在问 Telegram 要会话列表…"}))
}

/// 发一条测试消息（可选带一个本地文件的绝对路径，用来验证附件通道）。
fn push_test(path: &Path, body: &str) -> Result<Value> {
    use vca_platform::push::{make_pusher, send_with_retry, Provider, PushConfig, PushDoc};

    let v: Value = serde_json::from_str(body).unwrap_or_else(|_| json!({}));
    let s = vca_core::config::load_settings(path).unwrap_or_default();
    let id = s.push.provider.trim();
    if id.is_empty() {
        return Ok(json!({"ok": false, "error": "还没配置推送渠道"}));
    }
    let Some(provider) = Provider::parse(id) else {
        return Ok(json!({"ok": false, "error": format!("未知的推送渠道：{id}")}));
    };

    // 与 pipeline 完全一致的取值逻辑，避免"测试通过、真推送失败"
    let endpoint = if s.push.endpoint.trim().is_empty() {
        vca_core::secrets::get("VCA_PUSH_ENDPOINT")
    } else {
        s.push.endpoint.clone()
    };
    let cfg = PushConfig {
        provider,
        endpoint,
        token: vca_core::secrets::get("VCA_PUSH_TOKEN"),
        target: s.push.target.clone().unwrap_or_default(),
        target_type: s.push.target_type.clone(),
        app_id: vca_platform::push::credentials_for(provider).0,
        app_secret: vca_platform::push::credentials_for(provider).1,
        max_retries: 1,
        timeout_ms: 30_000,
        // 与真推送取同一个设置值，保证「测试通过 = 真推送也会这么做」
        image_card: s.push.image_card,
    };
    let attachment = v
        .get("attachment")
        .and_then(|x| x.as_str())
        .map(String::from);
    let doc = PushDoc {
        title: "VibeClassAgent 推送测试".to_string(),
        summary: "这是一条测试消息。收到它就说明推送链路是通的。".to_string(),
        docx: attachment.clone(),
        pdf: None,
        target: s.push.target.clone(),
        // 测试消息不带预览链接：它不是某一条具体作业
        preview_url: None,
    };

    let pusher = make_pusher(&cfg);
    let out = send_with_retry(pusher.as_ref(), &doc, 1);
    Ok(json!({
        "ok": out.success,
        "message_id": out.message_id,
        "error": out.error,
        "provider": id,
        "endpoint": cfg.endpoint,
        "target": cfg.target,
    }))
}

// ---------------------------------------------------------------- 通用配置

/// 确保有一个预览令牌。
///
/// 预览链接要发给群里的人，所以它得**长期有效** —— 不能用界面那种
/// 每次启动都变的令牌（daemon 根本拿不到它，链接就拼不出来）。
/// 生成一次就写进配置，之后一直用它。
fn ensure_preview_token(settings_path: &Path) {
    let s = vca_core::config::load_settings(settings_path).unwrap_or_default();
    if !s.ui.preview_token.trim().is_empty() {
        return;
    }
    // 与界面令牌同样的位混合：够随机，且不引 rand 依赖
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    let mut x = nanos ^ (pid << 64);
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    let token = format!("{x:032x}");
    let _ = set(settings_path, "ui.preview_token", &yaml_str(&token));
    tracing::info!("已生成预览令牌（内网预览链接会带它）");
}

fn get_general(path: &Path) -> Result<Value> {
    let s = vca_core::config::load_settings(path).unwrap_or_default();
    Ok(json!({
        "ok": true,
        "record": {
            "fps": s.record.fps,
            "height": s.record.height,
            "crf": s.record.crf,
            "screenshot_interval_secs": s.record.screenshot_interval_secs,
            "audio_source": s.record.audio_source,
        },
        "cleanup": { "retention_hours": s.cleanup.retention_hours },
        "overlay": { "enabled": s.overlay.enabled, "text": s.overlay.text },
        "ui": {
            "tray_icon": s.ui.tray_icon,
            "tray_badge": s.ui.tray_badge,
            // 开了之后界面服务绑全网卡，并把 preview_base 写进配置 ——
            // 同一个校园网里的手机就能打开预览页
            "allow_lan": s.ui.allow_lan,
            "preview_token": s.ui.preview_token,
        },
        // 企业微信：是否先推一张摘要卡片图
        "push": { "image_card": s.push.image_card, "preview_base": s.push.preview_base },
        "doc": { "formats": s.doc.formats, "max_screenshots": s.doc.max_screenshots },
    }))
}

fn post_general(path: &Path, body: &str) -> Result<Value> {
    let v: Value = serde_json::from_str(body)?;
    let mut wrote = Vec::new();
    let ints = [
        ("record.fps", "fps"),
        ("record.height", "height"),
        ("record.crf", "crf"),
        (
            "record.screenshot_interval_secs",
            "screenshot_interval_secs",
        ),
        ("cleanup.retention_hours", "retention_hours"),
        ("doc.max_screenshots", "max_screenshots"),
    ];
    for (key, field) in ints {
        if let Some(n) = v.get(field).and_then(|x| x.as_i64()) {
            if set(path, key, &n.to_string()) {
                wrote.push(field);
            }
        }
    }
    if let Some(a) = v.get("audio_source").and_then(|x| x.as_str()) {
        if set(path, "record.audio_source", &yaml_str(a)) {
            wrote.push("audio_source");
        }
    }
    if let Some(t) = v.get("overlay_text").and_then(|x| x.as_str()) {
        if set(path, "overlay.text", &yaml_str(t)) {
            wrote.push("overlay_text");
        }
    }
    if let Some(e) = v.get("overlay_enabled").and_then(|x| x.as_bool()) {
        if set(path, "overlay.enabled", if e { "true" } else { "false" }) {
            wrote.push("overlay_enabled");
        }
    }
    if let Some(t) = v.get("tray_icon").and_then(|x| x.as_bool()) {
        if set(path, "ui.tray_icon", if t { "true" } else { "false" }) {
            wrote.push("tray_icon");
        }
    }
    // 允许局域网访问：开了之后**必须重启界面服务**才会真的绑全网卡，
    // 所以界面那边会提示一句 —— 只写配置不改绑定，用户会以为没生效。
    if let Some(b) = v.get("allow_lan").and_then(|x| x.as_bool()) {
        if set(path, "ui.allow_lan", if b { "true" } else { "false" }) {
            wrote.push("allow_lan");
        }
        if b {
            ensure_preview_token(path);
        }
    }
    // 企业微信摘要卡片图
    if let Some(b) = v.get("image_card").and_then(|x| x.as_bool()) {
        if set(path, "push.image_card", if b { "true" } else { "false" }) {
            wrote.push("image_card");
        }
    }
    Ok(json!({ "ok": !wrote.is_empty(), "wrote": wrote }))
}

// ---------------------------------------------------------------- 课表 / 时间表

fn schedule_file(opts: &ServeOptions) -> PathBuf {
    opts.config_root.join("schedule").join("current.yaml")
}

fn timetable_file(opts: &ServeOptions) -> PathBuf {
    opts.config_root.join("timetable").join("current.yaml")
}

fn get_schedule(opts: &ServeOptions) -> Result<Value> {
    ensure_settings(opts, &settings_path(opts));
    let p = schedule_file(opts);
    if !p.is_file() {
        return Ok(json!({"ok": true, "exists": false, "entries": [], "teachers": []}));
    }
    let text = std::fs::read_to_string(&p)?;
    let f: vca_core::config::ScheduleFile =
        serde_yaml::from_str(&text).map_err(|e| anyhow::anyhow!("课表文件解析失败：{e}"))?;
    Ok(json!({
        "ok": true,
        "exists": true,
        "path": p.display().to_string(),
        "teachers": f.teachers,
        "entries": f.week_template.entries,
        "week_cycle": f.week_template.cycle,
        "weekend_entries": f
            .weekend_template
            .as_ref()
            .map(|w| w.entries.clone())
            .unwrap_or_default(),
        "overrides": f.overrides,
    }))
}

fn post_schedule(opts: &ServeOptions, body: &str) -> Result<Value> {
    let f: vca_core::config::ScheduleFile =
        serde_json::from_str(body).map_err(|e| anyhow::anyhow!("提交的课表格式不对：{e}"))?;
    let p = schedule_file(opts);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // 存一份存档再覆盖：改错了还能翻回来
    if p.is_file() {
        let arch = opts
            .config_root
            .join("schedule")
            .join("archive")
            .join(format!("current-{}.yaml", unix_secs()));
        let _ = std::fs::create_dir_all(arch.parent().unwrap_or(Path::new(".")));
        let _ = std::fs::copy(&p, &arch);
    }
    let text = serde_yaml::to_string(&f)?;
    std::fs::write(&p, text)?;

    // 回读确认
    let back: vca_core::config::ScheduleFile = serde_yaml::from_str(&std::fs::read_to_string(&p)?)?;
    let issues = vca_core::schedule::validate(None, &back.week_template);
    Ok(json!({
        "ok": true,
        "saved": p.display().to_string(),
        "entries": back.week_template.entries.len(),
        "issues": issues,
    }))
}

fn get_timetable(opts: &ServeOptions) -> Result<Value> {
    let p = timetable_file(opts);
    if !p.is_file() {
        return Ok(json!({"ok": true, "exists": false, "slots": []}));
    }
    let text = std::fs::read_to_string(&p)?;
    let tf: vca_core::config::TimetableFile =
        serde_yaml::from_str(&text).map_err(|e| anyhow::anyhow!("时间表文件解析失败：{e}"))?;
    let active = tf
        .timetables
        .iter()
        .find(|t| t.is_active)
        .or(tf.timetables.first());
    Ok(json!({
        "ok": true,
        "exists": true,
        "path": p.display().to_string(),
        "name": active.map(|t| t.name.clone()).unwrap_or_default(),
        "slots": active.map(|t| t.slots.clone()).unwrap_or_default(),
        "all": tf.timetables,
    }))
}

fn post_timetable(opts: &ServeOptions, body: &str) -> Result<Value> {
    let tf: vca_core::config::TimetableFile =
        serde_json::from_str(body).map_err(|e| anyhow::anyhow!("提交的时间表格式不对：{e}"))?;
    let p = timetable_file(opts);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if p.is_file() {
        let arch = opts
            .config_root
            .join("timetable")
            .join("archive")
            .join(format!("current-{}.yaml", unix_secs()));
        let _ = std::fs::create_dir_all(arch.parent().unwrap_or(Path::new(".")));
        let _ = std::fs::copy(&p, &arch);
    }
    let text = serde_yaml::to_string(&tf)?;
    std::fs::write(&p, text)?;

    // 顺带算一遍「由时间表推出的处理窗口」，让界面能立刻显示效果
    let active = tf.timetables.iter().find(|t| t.is_active);
    let windows = active
        .map(|t| vca_core::windows::derive_windows(t, vca_core::windows::WindowSpec::default()))
        .unwrap_or_default();
    Ok(json!({
        "ok": true,
        "saved": p.display().to_string(),
        "slots": active.map(|t| t.slots.len()).unwrap_or(0),
        "windows": windows,
    }))
}

// ---------------------------------------------------------------- 作业

/// 手动触发一条作业的处理进度。
struct JobRunState {
    running: bool,
    /// 正在跑哪一条。
    id: String,
    /// 最近一次的结果/进度。
    message: String,
    error: Option<String>,
}

static JOB_RUN: std::sync::Mutex<JobRunState> = std::sync::Mutex::new(JobRunState {
    running: false,
    id: String::new(),
    message: String::new(),
    error: None,
});

fn job_run_json() -> Value {
    let st = JOB_RUN.lock().unwrap_or_else(|e| e.into_inner());
    json!({
        "ok": true,
        "running": st.running,
        "id": st.id,
        "message": st.message,
        "error": st.error,
    })
}

/// 立即处理一条作业 —— 不等处理窗口、也不等错峰时段。
///
/// # 为什么要这个口子
///
/// 默认策略是**错峰**：DeepSeek 高峰时段全价、低峰半价，所以流水线会等。
/// 这是省钱的合理默认值，但它不该是**唯一**的选择 —— 用户可能下一节课就要用，
/// 或者刚配好推送想立刻看效果。「默认延迟」和「不许插队」是两回事。
///
/// 放后台线程：一条流水线要几十秒到几分钟，而本服务是单线程的，
/// 在请求线程里跑会把界面卡死（连"跑到哪一步了"都问不出来）。
fn job_run(opts: &ServeOptions, body: &str) -> Result<Value> {
    let v: Value = serde_json::from_str(body).unwrap_or_else(|_| json!({}));
    let id = v
        .get("id")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if id.is_empty() {
        return Ok(json!({"ok": false, "error": "没指定是哪一条作业"}));
    }
    {
        let st = JOB_RUN.lock().unwrap_or_else(|e| e.into_inner());
        if st.running {
            return Ok(json!({
                "ok": false,
                "error": format!("正在处理 {}，等它跑完再试", st.id),
            }));
        }
    }
    {
        let mut st = JOB_RUN.lock().unwrap_or_else(|e| e.into_inner());
        st.running = true;
        st.id = id.clone();
        st.message = "已开始，正在处理…".into();
        st.error = None;
    }

    let data_root = opts.data_root.clone();
    let config_root = opts.config_root.clone();
    let profile = opts.profile.clone();
    std::thread::spawn(move || {
        let done = |msg: String, err: Option<String>| {
            let mut st = JOB_RUN.lock().unwrap_or_else(|e| e.into_inner());
            st.running = false;
            st.message = msg;
            st.error = err;
        };

        let settings_path = config_root
            .join("profiles")
            .join(&profile)
            .join("settings.yaml");
        let settings = match vca_core::config::load_settings(&settings_path) {
            Ok(s) => s,
            Err(e) => return done(String::new(), Some(format!("读配置失败：{e}"))),
        };

        let layout = vca_core::paths::Layout::new(data_root, config_root);
        let store = vca_core::store::JobStore::new(layout.profile_data_dir(&profile));
        let mut job = match store.load(&id) {
            Ok(j) => j,
            Err(e) => return done(String::new(), Some(format!("找不到这条作业：{e}"))),
        };

        let python_dir = std::env::var("VCA_PYTHON_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("python"));

        let pipe =
            vca_engine::pipeline::Pipeline::new(&layout, &profile, &settings, &store, &python_dir);
        let out = pipe.process(&mut job);

        for s in &out.steps {
            tracing::info!("[手动] [{id}] {s}");
        }
        if let Some(e) = &out.error {
            tracing::warn!("[手动] [{id}] {e}");
            return done(String::new(), Some(e.clone()));
        }
        done(
            format!(
                "处理完成：{}",
                out.steps.last().cloned().unwrap_or_default()
            ),
            None,
        );
    });

    Ok(json!({"ok": true, "message": "已开始处理，可能要几分钟"}))
}

fn get_jobs(opts: &ServeOptions) -> Result<Value> {
    let layout = vca_core::paths::Layout::new(opts.data_root.clone(), opts.config_root.clone());
    let store = vca_core::store::JobStore::new(layout.profile_data_dir(&opts.profile));
    let jobs: Vec<Value> = store
        .list()
        .into_iter()
        .map(|j| {
            json!({
                "id": j.id,
                "course": j.course,
                "date": j.date.to_string(),
                "start": j.start.to_string(),
                "state": format!("{:?}", j.state),
                "last_error": j.last_error,
                "docx": j.docx_path,
            })
        })
        .collect();
    Ok(json!({ "ok": true, "jobs": jobs }))
}

/// 启动守护进程（界面上那个「开始工作」按钮）。
fn daemon_start(opts: &ServeOptions, body: &str) -> Result<Value> {
    let v: Value = serde_json::from_str(body).unwrap_or_else(|_| json!({}));
    let dry = v.get("dry_run").and_then(|x| x.as_bool()).unwrap_or(false);

    // 启动前先做一次配置自检：与其让它跑起来什么都不干，
    // 不如现在就说清楚缺什么 —— 用户在这个界面上才有机会改。
    let s = vca_core::config::load_settings(&settings_path(opts)).unwrap_or_default();
    let mut hints = Vec::new();
    if s.llm.base_url.trim().is_empty() || vca_platform::llm::is_placeholder(&s.llm.base_url) {
        hints.push("模型 API 还没配 —— 课后提取会降级成原文摘要");
    }
    if s.push.provider.trim().is_empty() {
        hints.push("推送渠道还没配 —— 文档只会存在本地");
    }
    let layout = vca_core::paths::Layout::new(opts.data_root.clone(), opts.config_root.clone());
    let sc = opts.config_root.join("schedule").join("current.yaml");
    let lessons = if sc.is_file() {
        vca_core::config::load_schedule_file(&sc).ok()
    } else {
        None
    };
    if lessons.is_none() {
        hints.push("课表还没导入 —— 不知道要录哪些课");
    }

    let mut r = crate::daemon::start(layout, &opts.profile, dry)?;
    if let Some(obj) = r.as_object_mut() {
        obj.insert("warnings".into(), json!(hints));
    }
    Ok(r)
}

/// 读日志尾部。
///
/// 只回最后 N 行：日志文件会一直长，整份塞给浏览器既慢又没用 ——
/// 排查问题时看的就是最近发生了什么。
fn read_logs(opts: &ServeOptions) -> Result<Value> {
    let path = opts.data_root.join("logs").join("ui.log");
    if !path.is_file() {
        return Ok(json!({
            "ok": true,
            "path": path.display().to_string(),
            "lines": [],
            "note": "还没有日志文件（程序启动后由界面写入）",
        }));
    }
    let text = std::fs::read_to_string(&path)?;
    let all: Vec<&str> = text.lines().collect();
    let take = 300.min(all.len());
    let tail: Vec<String> = all[all.len() - take..]
        .iter()
        .map(|s| s.to_string())
        .collect();
    Ok(json!({
        "ok": true,
        "path": path.display().to_string(),
        "total_lines": all.len(),
        "lines": tail,
    }))
}

/// 从 ClassIsland 导入课表与时间表。
///
/// 浏览器出于安全限制**拿不到用户选的文件的真实路径**，只能读到内容，
/// 所以这里收的是 JSON 文本，落一份临时文件再交给 `import_classisland`
/// （那个函数按路径工作，因为 CLI 那边就是传路径的）。
///
/// 导入器本身**全程只读**，不会碰 ClassIsland 的任何文件；写的是我们自己的
/// `timetable/current.yaml` 与 `schedule/current.yaml`，并且覆盖前先存档。
fn import_classisland(opts: &ServeOptions, body: &str) -> Result<Value> {
    let v: Value = serde_json::from_str(body)?;
    let text = v
        .get("json")
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow::anyhow!("没有收到文件内容"))?;
    if text.trim().is_empty() {
        return Ok(json!({ "ok": false, "error": "文件是空的" }));
    }
    let hint = v
        .get("timetable")
        .and_then(|x| x.as_str())
        .filter(|s| !s.trim().is_empty());

    let tmp = opts.data_root.join("logs").join("classisland-import.json");
    if let Some(d) = tmp.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&tmp, text)?;

    let imp = match vca_core::import::import_classisland(&tmp, hint) {
        Ok(x) => x,
        Err(e) => return Ok(json!({ "ok": false, "error": e.to_string() })),
    };

    // 覆盖前存档（与界面里手工保存走同一套逻辑）
    let stamp = unix_secs();
    for (sub, name, yaml) in [
        (
            "timetable",
            "current.yaml",
            serde_yaml::to_string(&wrap_timetable(&imp))?,
        ),
        (
            "schedule",
            "current.yaml",
            serde_yaml::to_string(&imp.schedule)?,
        ),
    ] {
        let dir = opts.config_root.join(sub);
        std::fs::create_dir_all(&dir)?;
        let cur = dir.join(name);
        if cur.is_file() {
            let arch = dir.join("archive").join(format!("current-{stamp}.yaml"));
            let _ = std::fs::create_dir_all(arch.parent().unwrap_or(Path::new(".")));
            let _ = std::fs::copy(&cur, &arch);
        }
        std::fs::write(&cur, yaml)?;
    }

    Ok(json!({
        "ok": true,
        "report": imp.report,
        "timetable_name": imp.timetable.name,
        "slots": imp.timetable.slots.len(),
        "entries": imp.schedule.week_template.entries.len(),
        "saved_to": opts.config_root.display().to_string(),
    }))
}

/// 把导入得到的时间表包成 `TimetableFile`（配置文件的外层结构）。
fn wrap_timetable(imp: &vca_core::import::ClassIslandImport) -> vca_core::config::TimetableFile {
    vca_core::config::TimetableFile {
        timetables: vec![imp.timetable.clone()],
    }
}

/// 界面文案：一次性把 `web.*` 整段给前端。
///
/// 界面有 200 多条文案，一条一发请求不现实；而且文案集中在语言文件里，
/// 改文案不必重新编译 exe。
fn i18n() -> Result<Value> {
    Ok(json!({
        "ok": true,
        "lang": vca_core::i18n::lang(),
        "strings": vca_core::i18n::subtree("web"),
    }))
}

/// 探测本机有没有跑机器人服务（NapCat / Lagrange / go-cqhttp）。
///
/// 只做 TCP 连接测试、不调 API —— 用户还没填 token 时，
/// 「这个端口上有没有东西在听」本身就是最有用的信息。
fn detect_bot() -> Result<Value> {
    const PORTS: &[u16] = &[3000, 3001, 6099, 5700, 8080, 12345];
    let mut found = Vec::new();
    for p in PORTS {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], *p));
        if std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300))
            .is_ok()
        {
            found.push(*p);
        }
    }
    Ok(json!({
        "ok": true,
        "found": found,
        "candidates": PORTS,
        "napcat_url": vca_platform::napcat::DOWNLOAD_PAGE,
    }))
}

// ---------------------------------------------------------------------------
// QQ 机器人（NapCat）：装 / 起 / 停 / 开目录
// ---------------------------------------------------------------------------

/// 当前由界面启动的 NapCat 进程 id。
///
/// 只放内存、不落盘：pid 只对「本进程刚拉起的那个」有意义。程序重启之后，
/// 原来那个 NapCat 可能还在后台跑——那时该看「OneBot 端口在不在听」，
/// 而不是相信一个从文件里读出来、可能早就过期的 pid。
static NAPCAT_PID: std::sync::Mutex<Option<u32>> = std::sync::Mutex::new(None);

/// 后台安装任务的进度。
///
/// 为什么安装必须挪到后台线程：下载几十 MB 加解压要几十秒，
/// 而本服务是**单线程**的（tiny_http 在主循环里同步处理请求）。
/// 直接在请求线程里下载，整个界面会卡住——连「装完了吗」这种查询都发不出去。
struct InstallState {
    running: bool,
    message: String,
    error: Option<String>,
}

static INSTALL: std::sync::Mutex<InstallState> = std::sync::Mutex::new(InstallState {
    running: false,
    message: String::new(),
    error: None,
});

/// 取安装状态。忽略「上次持锁线程 panic」这种毒化——里面只是给人看的进度，
/// 没必要因为它把整个界面带崩（毒化后 into_inner 仍可正常读写）。
fn install_state() -> std::sync::MutexGuard<'static, InstallState> {
    INSTALL.lock().unwrap_or_else(|e| e.into_inner())
}

fn set_install(running: bool, message: &str, error: Option<String>) {
    let mut st = install_state();
    st.running = running;
    st.message = message.to_string();
    st.error = error;
}

/// 从 OneBot 地址里抠端口；抠不到就退回 OneBot 生态的默认值 3000。
///
/// 单独写成函数是为了能测：`push.endpoint` 是用户手填的字符串，
/// 写成 `127.0.0.1` 不带端口、写成 `localhost:3000/` 带路径都可能出现。
fn parse_port(endpoint: &str) -> Option<u16> {
    let rest = endpoint.split("//").nth(1).unwrap_or(endpoint);
    let host = rest.split(['/', '?']).next()?;
    if !host.contains(':') {
        return None;
    }
    host.rsplit(':').next()?.parse::<u16>().ok()
}

/// 读文件尾部若干字符（日志可能很长，界面只要最后一段）。
fn read_tail(path: &Path, max_chars: usize) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let n = text.chars().count();
    if n <= max_chars {
        return text;
    }
    text.chars().skip(n - max_chars).collect()
}

/// 机器人当前状态：装没装、跑没跑、服务通没通、日志尾巴。
fn bot_status(settings: &Path) -> Result<Value> {
    let root = vca_platform::napcat::root();
    let nc = vca_platform::napcat::detect(&root);

    let pid = *NAPCAT_PID.lock().unwrap_or_else(|e| e.into_inner());
    let running = pid.map(vca_platform::napcat::is_alive).unwrap_or(false);

    let ep = vca_core::config::load_settings(settings)
        .unwrap_or_default()
        .push
        .endpoint;
    let port = parse_port(&ep).unwrap_or(3000);

    let st = install_state();
    // 进程不在了就别把 pid 报出去：界面上显示一个已经死掉的 pid 只会让人困惑
    let live_pid = if running { pid } else { None };
    Ok(json!({
        "ok": true,
        "root": root.display().to_string(),
        "installed": nc.is_some(),
        "flavor": nc.as_ref().map(|n| n.flavor.label()),
        "version": nc.as_ref().and_then(|n| n.version.clone()),
        "entry": nc.as_ref().map(|n| n.entry.display().to_string()),
        "pid": live_pid,
        "running": running,
        "port": port,
        "port_open": vca_platform::napcat::port_open(port),
        "log": read_tail(&vca_platform::napcat::launcher_log_at(&root), 4000),
        "installing": st.running,
        "install_message": st.message,
        "install_error": st.error,
        "download_page": vca_platform::napcat::DOWNLOAD_PAGE,
        "sources": vca_platform::napcat::sources()
            .into_iter()
            .map(|(label, url)| json!({"label": label, "url": url}))
            .collect::<Vec<_>>(),
    }))
}

/// 一键安装：后台下载官方 Shell 包 → 解压到安装目录。
///
/// **立刻返回**，真正的活在后台线程里跑；前端靠轮询 [`bot_status`] 看进度。
fn bot_install(body: &str) -> Result<Value> {
    if install_state().running {
        return Ok(json!({"ok": false, "error": "上一次安装还没结束，等它跑完再试"}));
    }

    // source: "auto"（默认，依次试）或 "0"/"1"/"2"（指定某一个源）。
    // 让用户能指定，是因为自动模式在校园网下要等前一个源超时才轮到镜象，
    // 而用户往往一开始就知道该走哪个。
    let v: Value = serde_json::from_str(body).unwrap_or_else(|_| json!({}));
    let only = match v.get("source").and_then(|x| x.as_str()).unwrap_or("auto") {
        "" | "auto" => None,
        s => s
            .parse::<usize>()
            .ok()
            .filter(|i| *i < vca_platform::napcat::sources().len()),
    };

    set_install(true, "正在下载…", None);

    std::thread::spawn(move || {
        let root = vca_platform::napcat::root();
        let zip = vca_platform::napcat::temp_zip_path();
        let outcome =
            vca_platform::napcat::download_shell_zip(&zip, 600_000, only).and_then(|url| {
                vca_platform::napcat::install_from_zip(&zip, &root).map(|nc| (url, nc))
            });
        // 不管成没成，临时包都不留：失败时留着只会让人下次装的时候多占几十 MB
        let _ = std::fs::remove_file(&zip);

        match outcome {
            Ok((url, nc)) => {
                let v = nc.version.unwrap_or_else(|| "未知版本".to_string());
                tracing::info!("NapCat 安装完成：{v}（来源 {url}）");
                set_install(false, &format!("已装好 {v}"), None);
            }
            Err(e) => {
                tracing::warn!("NapCat 安装失败：{e}");
                set_install(false, "", Some(e.to_string()));
            }
        }
    });

    Ok(json!({"ok": true, "message": "已开始下载，装好后会自动刷新"}))
}

/// 启动机器人（已经在跑就先停掉，免得两个实例抢同一个 QQ 登录）。
fn bot_start() -> Result<Value> {
    let root = vca_platform::napcat::root();
    let nc = vca_platform::napcat::detect(&root).ok_or_else(|| {
        anyhow::anyhow!(
            "还没装好机器人（看的是 {}），先点「一键安装」",
            root.display()
        )
    })?;

    let old = *NAPCAT_PID.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(p) = old {
        if vca_platform::napcat::is_alive(p) {
            vca_platform::napcat::stop(p)?;
        }
    }

    let pid = vca_platform::napcat::start(&nc)?;
    *NAPCAT_PID.lock().unwrap_or_else(|e| e.into_inner()) = Some(pid);
    tracing::info!("已启动 NapCat，pid={pid}");
    Ok(json!({"ok": true, "pid": pid, "message": format!("已启动（pid {pid}）")}))
}

/// 停止机器人。
fn bot_stop() -> Result<Value> {
    let pid = *NAPCAT_PID.lock().unwrap_or_else(|e| e.into_inner());
    let Some(p) = pid else {
        return Ok(json!({"ok": true, "message": "它本来就没在跑"}));
    };
    vca_platform::napcat::stop(p)?;
    *NAPCAT_PID.lock().unwrap_or_else(|e| e.into_inner()) = None;
    Ok(json!({"ok": true, "message": "已停止"}))
}

/// 打开安装目录（没装过也打开——用户手动下载之后正需要往里放）。
fn bot_open_dir() -> Result<Value> {
    let root = vca_platform::napcat::root();
    std::fs::create_dir_all(&root)?;
    let _ = std::process::Command::new("explorer").arg(&root).spawn();
    Ok(json!({"ok": true}))
}

/// 退出程序。
///
/// 为什么需要这个接口：浏览器窗口是 `--app` 拉起来的独立进程，把它关掉之后
/// **后台服务还在跑** —— 用户看到窗口没了会以为程序退了，实际它还占着端口。
/// 与其让人去任务管理器里杀进程，不如给一个明确的出口。
fn quit() -> Result<Value> {
    tracing::info!("收到退出请求，界面服务即将关闭");
    std::thread::spawn(|| {
        // 留一点时间把响应发出去，否则浏览器那边看到的是连接被重置
        std::thread::sleep(std::time::Duration::from_millis(300));
        std::process::exit(0);
    });
    Ok(json!({ "ok": true }))
}

/// Unix 秒（给存档文件名用）。
fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 拉取服务端可用的模型列表。
///
/// 允许临时传入一次性的 base_url / api_key：用户刚填完还没保存就想看看有什么模型，
/// 这时不该强迫他先存一次。
fn list_models(path: &Path, body: &str) -> Result<Value> {
    use vca_platform::llm::{list_models as fetch, LlmConfig, LlmMode};

    let v: Value = serde_json::from_str(body).unwrap_or_else(|_| json!({}));
    let s = vca_core::config::load_settings(path).unwrap_or_default();

    let base = v
        .get("base_url")
        .and_then(|x| x.as_str())
        .filter(|x| !x.trim().is_empty())
        .map(String::from)
        .unwrap_or_else(|| s.llm.base_url.clone());
    let key = v
        .get("api_key")
        .and_then(|x| x.as_str())
        .filter(|x| !x.trim().is_empty())
        .map(String::from)
        .unwrap_or_else(|| vca_core::secrets::get("VCA_LLM_API_KEY"));

    if base.trim().is_empty() || key.trim().is_empty() {
        return Ok(json!({
            "ok": false,
            "error": "要拉模型列表，得先填接口地址和 API Key",
        }));
    }

    let cfg = LlmConfig {
        mode: LlmMode::PaidApi,
        provider: s.llm.provider.clone(),
        base_url: base,
        api_key: key,
        model: s.llm.model.clone(),
        timeout_ms: 20_000,
        max_chars: 8000,
        browser: Default::default(),
    };

    match fetch(&cfg) {
        Ok(list) => Ok(json!({ "ok": true, "models": list })),
        Err(e) => Ok(json!({ "ok": false, "error": e.to_string() })),
    }
}

/// 预览能清理掉哪些录像（**只算不删**）。
fn clean_preview(opts: &ServeOptions) -> Result<Value> {
    let layout = vca_core::paths::Layout::new(opts.data_root.clone(), opts.config_root.clone());
    let now = vca_platform::clock::now_local();
    let rep = vca_core::cleanup::sweep(&layout.profile_data_dir(&opts.profile), now, true);
    Ok(json!({
        "ok": true,
        "scanned": rep.scanned,
        "deleting": rep.deleted.len(),
        "skipped": rep.skipped.len(),
        "failed": rep.failed.len(),
    }))
}

#[cfg(test)]
mod bot_tests {
    use super::*;

    #[test]
    fn parses_port_from_common_endpoint_forms() {
        assert_eq!(parse_port("http://127.0.0.1:3000"), Some(3000));
        assert_eq!(parse_port("http://localhost:5700/"), Some(5700));
        assert_eq!(parse_port("127.0.0.1:6099"), Some(6099));
        assert_eq!(parse_port("http://127.0.0.1:3000/api?x=1"), Some(3000));
    }

    #[test]
    fn falls_back_when_no_port_present() {
        // 没写端口时不该瞎猜出一个 "1"（把整段字符串 parse 成端口的经典错法）
        assert_eq!(parse_port("http://127.0.0.1"), None);
        assert_eq!(parse_port(""), None);
        assert_eq!(parse_port("http://[::1]:3000"), Some(3000));
    }

    #[test]
    fn tail_returns_whole_text_when_short() {
        let d = std::env::temp_dir().join(format!("vca-tail-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("t.log");
        std::fs::write(&f, "hello").unwrap();
        assert_eq!(read_tail(&f, 100), "hello");
        // 按**字符**截，不能按字节：日志里有中文，按字节切会切出半个字
        std::fs::write(&f, "一二三四五").unwrap();
        assert_eq!(read_tail(&f, 3), "三四五");
        assert_eq!(read_tail(&d.join("nope.log"), 10), "");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn install_state_is_readable_after_poisoning() {
        // 毒化之后仍要能读：这条状态只影响一个卡片，不该把整个界面带崩
        let _ = std::thread::spawn(|| {
            let _g = INSTALL.lock().unwrap();
            panic!("故意 panic，制造 poisoned 锁");
        })
        .join();
        set_install(false, "ok", None);
        assert!(!install_state().running);
    }
}
