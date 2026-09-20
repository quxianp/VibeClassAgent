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
        ("GET", "/api/config/general") => get_general(&settings_path),
        ("POST", "/api/config/general") => post_general(&settings_path, &body),
        ("GET", "/api/schedule") => get_schedule(opts),
        ("POST", "/api/schedule") => post_schedule(opts, &body),
        ("GET", "/api/timetable") => get_timetable(opts),
        ("POST", "/api/timetable") => post_timetable(opts, &body),
        ("GET", "/api/jobs") => get_jobs(opts),
        ("POST", "/api/models") => list_models(&settings_path, &body),
        ("POST", "/api/clean/preview") => clean_preview(opts),
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
            "key_set": !std::env::var("VCA_LLM_API_KEY").unwrap_or_default().is_empty(),
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
        "key_set": !std::env::var("VCA_LLM_API_KEY").unwrap_or_default().is_empty(),
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
            std::env::set_var("VCA_LLM_API_KEY", k.trim());
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
    Ok(json!({
        "ok": true,
        "provider": s.push.provider,
        "endpoint": s.push.endpoint,
        "target": s.push.target.clone().unwrap_or_default(),
        "target_type": s.push.target_type,
        "max_retries": s.push.max_retries,
        // 同样只回"设没设"，不回内容
        "token_set": !std::env::var("VCA_PUSH_TOKEN").unwrap_or_default().is_empty(),
        "qq_appid_set": !std::env::var("VCA_QQ_APP_ID").unwrap_or_default().is_empty(),
        "qq_secret_set": !std::env::var("VCA_QQ_APP_SECRET").unwrap_or_default().is_empty(),
    }))
}

fn post_push(path: &Path, body: &str) -> Result<Value> {
    let v: Value = serde_json::from_str(body)?;
    let mut wrote = Vec::new();

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
    ] {
        if let Some(val) = v.get(field).and_then(|x| x.as_str()) {
            if !val.trim().is_empty() {
                vca_core::secrets::upsert(&sp, env, val.trim())?;
                std::env::set_var(env, val.trim());
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
        std::env::var("VCA_PUSH_ENDPOINT").unwrap_or_default()
    } else {
        s.push.endpoint.clone()
    };
    let cfg = PushConfig {
        provider,
        endpoint,
        token: std::env::var("VCA_PUSH_TOKEN").unwrap_or_default(),
        target: s.push.target.clone().unwrap_or_default(),
        target_type: s.push.target_type.clone(),
        app_id: std::env::var("VCA_QQ_APP_ID").unwrap_or_default(),
        app_secret: std::env::var("VCA_QQ_APP_SECRET").unwrap_or_default(),
        max_retries: 1,
        timeout_ms: 30_000,
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
        .unwrap_or_else(|| std::env::var("VCA_LLM_API_KEY").unwrap_or_default());

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
