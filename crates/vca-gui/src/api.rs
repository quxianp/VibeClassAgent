//! 界面用的后端接口。
//!
//! 所有写操作都走 `vca_core::setup::patch_settings_line`（按 `a.b.c` 逐级定位），
//! **不要自己拼 YAML** —— 那正是上一轮「配好的模型重启就没了」的根因所在。
//! 一句话：这里只做参数校验和格式转换，落盘的事交给那一个函数。

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{json, Value};

use crate::server::ServeOptions;

/// 请求体上限（字节）。
///
/// 这些接口全是「改配置 / 点按钮」，JSON 再大也就几十 KB。
/// 旧实现无脑 `read_to_string`，一个几 GB 的 POST 就能把界面进程的内存吃光
/// （评审 R-20）—— 而且它绑在本机回环上，同机任何程序都能发。
///
/// 8 MB 留了很大余量：课表导入（ClassIsland 导出文件）是这里最大的载荷，
/// 一个几千节课的学期表也远不到这个数。
const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;

/// 读请求体，超过上限就拒绝。
///
/// 先看 `Content-Length`：能提前拒绝的就不必读进内存。
/// 没有该头（chunked）时边读边数，超了就停。
fn read_body_limited(req: &mut tiny_http::Request) -> std::result::Result<String, String> {
    use std::io::Read;

    if let Some(len) = req.body_length() {
        if len as u64 > MAX_BODY_BYTES {
            return Err(format!("请求体过大（{len} 字节，上限 {MAX_BODY_BYTES}）"));
        }
    }

    let mut body = String::new();
    // take 一个字节的多余量：读到 MAX+1 就能判断"超了"，避免无界读取
    let mut limited = req.as_reader().take(MAX_BODY_BYTES + 1);
    limited
        .read_to_string(&mut body)
        .map_err(|e| format!("读取请求体失败: {e}"))?;
    if body.len() as u64 > MAX_BODY_BYTES {
        return Err(format!("请求体过大（上限 {MAX_BODY_BYTES} 字节）"));
    }
    Ok(body)
}

/// 按路径分发。
///
/// 用 `query` 参数名（不是 `_query`）：日志接口要用它做增量游标。
pub fn dispatch(
    mut req: tiny_http::Request,
    path: &str,
    query: &str,
    opts: &ServeOptions,
) -> Result<()> {
    let method = req.method().as_str().to_uppercase();
    let mut body = String::new();
    if method == "POST" {
        match read_body_limited(&mut req) {
            Ok(b) => body = b,
            Err(e) => {
                // 413 是给"太大了"用的；客户端（本项目的 app.js）会读出
                // error 字段显示给人看
                return crate::server::respond_json(
                    req,
                    413,
                    &json!({"ok": false, "error": e}).to_string(),
                );
            }
        }
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
        ("POST", "/api/push/discover/stop") => push_discover_stop(),
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
        ("GET", "/api/deps") => Ok(deps_status(opts)),
        // 下载源自定义：让用户自己填镜像。
        // 没有一组内置地址能在所有网络里都通（教育网/内网/自建镜像），
        // 写死地址的后果是"在某些学校永远装不上"。
        ("GET", "/api/deps/mirrors") => Ok(deps_mirrors_get(opts)),
        ("POST", "/api/deps/mirrors") => deps_mirrors_set(opts, &body),
        // 测一个地址通不通：填完就能立刻知道对不对，
        // 不用等真的下载到一半才发现（那样要等超时，很难判断是自己填错还是网络差）。
        ("POST", "/api/deps/mirrors/test") => deps_mirrors_test(&body),
        ("POST", "/api/deps/fetch") => deps_fetch(opts, &body),
        ("GET", "/api/deps/fetch") => Ok(deps_fetch_json()),
        ("POST", "/api/deps/dismiss") => Ok(deps_dismiss(opts, &body)),
        ("GET", "/api/logs") => read_logs(opts, query),
        ("POST", "/api/logs/clear") => clear_logs(),
        ("GET", "/api/i18n") => i18n(),
        ("POST", "/api/detect-bot") => detect_bot(),
        ("GET", "/api/bot/napcat") => bot_status(&settings_path),
        ("POST", "/api/bot/napcat/install") => bot_install(&body),
        ("POST", "/api/bot/napcat/start") => bot_start(),
        ("POST", "/api/bot/napcat/stop") => bot_stop(),
        ("POST", "/api/bot/napcat/open") => bot_open_dir(),
        ("POST", "/api/bot/napcat/configure") => bot_configure(opts, &body),
        ("POST", "/api/import/classisland") => import_classisland(opts, &body),
        ("POST", "/api/quit") => quit(),
        _ => {
            // 路由没匹配上：这是 404，不是"业务失败"
            return crate::server::respond_json(
                req,
                404,
                &json!({"ok": false, "error": format!("没有这个接口：{method} {path}")})
                    .to_string(),
            );
        }
    };

    // 状态码要分清「接口本身出错」和「业务失败」（评审 R-10）。
    //
    // 旧实现无论什么情况都回 200，只在 body 里写 ok:false。后果是：
    // 浏览器 devtools、任何 HTTP 客户端、将来可能出现的脚本化调用
    // 全都看不出失败 —— 得先解析 JSON 才知道，而 200 又暗示"一切正常"。
    //
    // 这里把 handler 报的 Err 一律当 500（它们都是内部错误：文件读写、
    // 配置解析、网络调用抛上来的），而**业务上的校验失败由 handler
    // 自己返回 `{ok:false}` 并保持 200** —— 那类"用户填错了"是正常结果，
    // 前端按 ok 字段处理。
    match result {
        Ok(v) => crate::server::respond_json(req, 200, &v.to_string()),
        Err(e) => {
            tracing::warn!("接口 {method} {path} 出错: {e}");
            crate::server::respond_json(
                req,
                500,
                &json!({"ok": false, "error": e.to_string()}).to_string(),
            )
        }
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
    /// 用户点了「停止」——监听线程下一轮就收工。
    stop: bool,
    chats: Vec<(String, String)>,
    /// 认不出的帧样例（官方改字段名时用来定位）。
    unknown: Vec<String>,
    error: Option<String>,
    note: String,
}

static DISCOVER: std::sync::Mutex<DiscoverState> = std::sync::Mutex::new(DiscoverState {
    running: false,
    stop: false,
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
        .unwrap_or("wecom-aibot")
        .to_string();

    {
        let mut st = discover_state();
        st.running = true;
        st.stop = false;
        st.chats.clear();
        st.unknown.clear();
        st.error = None;
        st.note = String::new();
    }

    // 默认监听 5 分钟。原来的 15 秒是设计失误：企业微信的回调是**实时**推送、
    // 不补发历史，用户得恰好在那 15 秒内对机器人说话才拿得到 —— 实际几乎不可能。
    let listen = v.get("seconds").and_then(|x| x.as_u64()).unwrap_or(300);

    if provider == "wecom-aibot" {
        let bot_id = pick(&v, "wecom_bot_id", "VCA_WECOM_BOT_ID");
        let secret = pick(&v, "wecom_bot_secret", "VCA_WECOM_BOT_SECRET");
        std::thread::spawn(move || {
            let cfg = vca_platform::wecom_aibot::AibotConfig::new(&bot_id, &secret, "", "");
            // 边听边把新会话写进共享状态：前端轮询就能看到实时进展，
            // 不用等监听结束才知道有没有收获。
            let found = vca_platform::wecom_aibot::listen_chats(
                &cfg,
                listen,
                || {
                    let st = discover_state();
                    st.stop
                },
                |chats| {
                    let mut st = discover_state();
                    st.chats = chats.to_vec();
                },
            );
            let mut st = discover_state();
            match found {
                Ok(list) => {
                    st.chats = list.chats;
                    st.unknown = list.unknown_frames;
                    st.note = if st.chats.is_empty() {
                        "这段时间里没有收到任何会话消息。企业微信只在**有人对机器人说话的那一刻**\
                         把会话 id 推过来，不补发历史 —— 所以请在监听期间去群里 @ 一次机器人\
                         （或给它发一条私聊），再回来看这里。"
                            .to_string()
                    } else {
                        "这些就是机器人最近互动过的会话。点「填入」即可。".to_string()
                    };
                }
                Err(e) => st.error = Some(e.to_string()),
            }
            st.running = false;
        });
        return Ok(json!({
            "ok": true,
            "message": format!("正在监听（最多 {} 秒）——现在去群里 @ 一下机器人", listen),
        }));
    }

    // 只有企微智能机器人需要「找会话」（它要一个 chatid）。
    // QQ 侧的两个渠道都不需要：OneBot 填群号、QQ 官方填群 openid。
    Ok(json!({"ok": false, "error": "这个渠道没有「获取会话」这种方式"}))
}

/// 让正在跑的会话监听提前收工（用户已经拿到想要的会话了）。
fn push_discover_stop() -> Result<Value> {
    let mut st = discover_state();
    if !st.running {
        return Ok(json!({"ok": true, "message": "本来就没在监听"}));
    }
    st.stop = true;
    Ok(json!({"ok": true, "message": "已请求停止"}))
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

/// 确保预览令牌存在；返回当前令牌。
///
/// 预览链接要发给群里的人，所以它得**长期有效** —— 不能用界面那种
/// 每次启动都变的令牌（daemon 根本拿不到它，链接就拼不出来）。
/// 生成一次就写进配置，之后一直用它。
///
/// 用 [`vca_core::token`] 的 CSPRNG：这个令牌会出现在 URL、聊天记录和
/// 浏览器历史里，是三个令牌里最需要抗猜测的一个（评审 R-11）。
/// 返回 `String` 而不是 `()`，是为了让调用方能确认"现在确实有一个令牌"。
pub(crate) fn ensure_preview_token(settings_path: &Path) -> String {
    let s = vca_core::config::load_settings(settings_path).unwrap_or_default();
    let existing = s.ui.preview_token.trim().to_string();
    if !existing.is_empty() {
        return existing;
    }
    let token = vca_core::token::generate();
    let _ = set(settings_path, "ui.preview_token", &yaml_str(&token));
    tracing::info!("已生成预览令牌（预览链接会带它）");
    token
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
            // allow_lan / preview_token 不再对界面暴露：
            // 局域网绑定从未实现（评审 R-08），而预览令牌属于服务端细节，
            // 回显它只会让 token 多一份泄漏面（评审 R-11）。
            // 字段仍留在配置里以兼容旧文件，但不参与界面读写。
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
    // allow_lan 已不再由界面写入：服务端从未实现 LAN 绑定（评审 R-08），
    // 写进去只会让用户以为生效了。字段留在配置里兼容旧文件，但不接受界面修改。
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

/// 读时间表文件（不存在就返回空表）。
fn load_timetables(opts: &ServeOptions) -> Vec<vca_core::model::Timetable> {
    vca_core::config::load_timetables(&timetable_file(opts))
}

/// 读课表文件并归一化（不存在返回 None）。
///
/// 顺便把旧格式就地升级成新格式并留一份 `.bak` ——
/// 升级意味着用户以后不用再人肉对着时间表抄起止时间。
fn load_schedule(opts: &ServeOptions) -> Option<vca_core::config::ScheduleFile> {
    let p = schedule_file(opts);
    let tts = load_timetables(opts);
    if let Some(bak) = vca_core::import::upgrade_schedule_file(&p, &tts) {
        tracing::info!("课表已升级为新格式，原文件备份在 {}", bak.display());
    }
    let text = std::fs::read_to_string(&p).ok()?;
    let mut f: vca_core::config::ScheduleFile = serde_yaml::from_str(&text).ok()?;
    f.normalize(&tts);
    Some(f)
}

fn get_schedule(opts: &ServeOptions) -> Result<Value> {
    ensure_settings(opts, &settings_path(opts));
    let tts = load_timetables(opts);
    let Some(mut f) = load_schedule(opts) else {
        return Ok(json!({"ok": true, "exists": false, "entries": [], "teachers": []}));
    };
    f.normalize(&tts);
    let active = tts.iter().find(|t| t.is_active).or(tts.first());
    // 上课时间点清单：界面上「课表」是按时间点排课程格，需要它当列头
    let class_slots: Vec<Value> = active
        .map(|t| {
            t.class_slots()
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    json!({
                        "index": i,
                        "period": s.period,
                        "start": s.start,
                        "end": s.end,
                        "duration": s.duration_minutes(),
                        "default_subject": s.default_subject,
                        "is_hidden": s.is_hidden,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({
        "ok": true,
        "exists": true,
        "path": schedule_file(opts).display().to_string(),
        "teachers": f.teachers,
        "entries": f.week_template.entries,
        "week_cycle": f.week_template.cycle,
        "weekend_entries": f
            .weekend_template
            .as_ref()
            .map(|w| w.entries.clone())
            .unwrap_or_default(),
        "overrides": f.overrides,
        // 与 ClassIsland 一样，把「这份课表绑的时间表 / 触发规则」一并给界面，
        // 界面才有东西可编辑（而不是只能改一堆隐式约定）
        "time_layout_id": f.week_template.time_layout_id,
        "time_rule": f.week_template.time_rule,
        "is_enabled": f.week_template.is_enabled,
        "class_slots": class_slots,
        "time_layouts": tts
            .iter()
            .map(|t| json!({"id": t.id, "name": t.name, "is_active": t.is_active}))
            .collect::<Vec<_>>(),
    }))
}

/// 保存课表。
///
/// # 顺序很重要：先校验，再落盘
///
/// 旧实现是「写盘 → 回读 → 校验 → 把 issues 放进响应」（评审 R-09）。
/// 也就是说**校验失败也已经写进去了**：用户看到一排红字，
/// 以为"没保存成功"，实际磁盘上已经是被改坏的课表 —— 关掉页面再打开，
/// 坏数据还在，而且下一节课就会按它去录。
///
/// 现在的顺序：
/// 1. 解析提交的 JSON；
/// 2. normalize + 校验（含时间表绑定）；
/// 3. **有错就原样返回，一个字节都不写**；
/// 4. 没问题才原子替换（先写临时文件再 rename）。
///
/// 第 4 步用 rename 而不是 `fs::write` 直接覆盖：写到一半断电/崩溃时，
/// 旧文件要么完整保留、要么被完整的新的替换，不会出现半个 YAML。
fn post_schedule(opts: &ServeOptions, body: &str) -> Result<Value> {
    let mut f: vca_core::config::ScheduleFile =
        serde_json::from_str(body).map_err(|e| anyhow::anyhow!("提交的课表格式不对：{e}"))?;
    let tts = load_timetables(opts);
    f.normalize(&tts);

    // 用**绑定的**时间表校验，而不是 active：多份作息时两者可能不同（R-02）
    let bound = vca_core::config::ScheduleFile::resolve_timetable(&f.week_template, &tts);
    let mut issues = vca_core::schedule::validate(bound, &f.week_template);
    // 与 ClassIsland 一致：课表必须引用一个存在的时间表。
    // 时间表一个都没有时不算错 —— 用户可能就是先排课再补作息。
    if !f.week_template.time_layout_id.is_empty() && bound.is_none() {
        issues.push(format!(
            "课表绑定的时间表 {} 不存在",
            f.week_template.time_layout_id
        ));
    }

    let p = schedule_file(opts);
    if !issues.is_empty() {
        // 只报告，不写盘。文件保持原样，用户改完再提交一次。
        tracing::info!("课表校验未通过（{} 项），已放弃保存", issues.len());
        return Ok(json!({
            "ok": false,
            "saved": Value::Null,
            "entries": f.week_template.entries.len(),
            "issues": issues,
            "error": "课表没有通过校验，未保存（文件保持原样）",
        }));
    }

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
    write_atomic(&p, &text)?;

    Ok(json!({
        "ok": true,
        "saved": p.display().to_string(),
        "entries": f.week_template.entries.len(),
        "issues": Vec::<String>::new(),
    }))
}

/// 原子写文本文件：先写同目录下的临时文件，再 rename 覆盖目标。
///
/// 同目录是必须的 —— 跨盘 rename 会退化成复制，也就失去了原子性。
///
/// Windows 上 `fs::rename` 覆盖已存在的文件是允许的（用的是
/// `MoveFileEx` 语义），所以这里不需要先删目标。
///
/// 临时文件名带 pid 与纳秒：同一目录下多次写入不会互相踩。
fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "out".into());
    let tmp = dir.join(format!(
        ".{name}.tmp{}-{}",
        std::process::id(),
        unix_nanos()
    ));
    std::fs::write(&tmp, text)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        // rename 失败时别把垃圾留在目录里
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow::anyhow!(
            "写入 {} 失败（原文件未改动）: {e}",
            path.display()
        ));
    }
    Ok(())
}

/// 当前纳秒时间戳，仅用于生成不重复的临时文件名。
fn unix_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn get_timetable(opts: &ServeOptions) -> Result<Value> {
    let p = timetable_file(opts);
    if !p.is_file() {
        return Ok(json!({"ok": true, "exists": false, "slots": [], "all": []}));
    }
    let tf = vca_core::config::load_timetable_file(&p)
        .map_err(|e| anyhow::anyhow!("时间表文件解析失败：{e}"))?;
    let active = tf
        .timetables
        .iter()
        .find(|t| t.is_active)
        .or(tf.timetables.first());
    // 每个时间点带上时长：界面上「时长」是与起止时间并列的一列（对齐 ClassIsland
    // 时间轴视图里的 Duration），让前端自己算一遍没有意义
    let slots: Vec<Value> = active
        .map(|t| {
            t.slots
                .iter()
                .map(|s| {
                    let mut v = serde_json::to_value(s).unwrap_or_else(|_| json!({}));
                    if let Some(o) = v.as_object_mut() {
                        o.insert("duration".into(), json!(s.duration_minutes()));
                    }
                    v
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({
        "ok": true,
        "exists": true,
        "path": p.display().to_string(),
        "id": active.map(|t| t.id.clone()).unwrap_or_default(),
        "name": active.map(|t| t.name.clone()).unwrap_or_default(),
        "slots": slots,
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
    // 时间点重叠要当场说 —— 两份重叠的作息会让「这一节到底几点下课」变得没答案
    let issues = active.map(|t| t.issues()).unwrap_or_default();
    Ok(json!({
        "ok": true,
        "saved": p.display().to_string(),
        "slots": active.map(|t| t.slots.len()).unwrap_or(0),
        "windows": windows,
        "issues": issues,
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
    let has_lessons = load_schedule(opts)
        .map(|f| !f.week_template.entries.is_empty())
        .unwrap_or(false);
    if !has_lessons {
        hints.push("课表还没导入 —— 不知道要录哪些课");
    }

    let mut r = crate::daemon::start(layout, &opts.profile, dry)?;
    if let Some(obj) = r.as_object_mut() {
        obj.insert("warnings".into(), json!(hints));
    }
    Ok(r)
}

/// 读日志。
///
/// # 为什么改读内存缓冲
///
/// 旧实现每次读整个 `logs/ui.log` 再截尾 —— 界面轮询时这是纯浪费，
/// 而且控制台窗口隐藏后，**文件是唯一来源**（原先还能看黑框）。
/// 现在改成读 [`vca_platform::logbuf`] 的环形缓冲：
///
/// - `since` 参数给游标时只返回新增行，界面轮询的开销与日志总量无关；
/// - 结构化的级别/时间分离返回，界面能按级别上色和筛选；
/// - 不依赖文件存在（日志文件写失败时界面仍然有得看）。
///
/// `all=1` 时返回当前缓冲的全部内容（用于"清空后重新载入"）。
fn read_logs(opts: &ServeOptions, query: &str) -> Result<Value> {
    // 游标：界面上次拿到的最大 seq。缺失表示"给我最新的"
    let since = query_param(query, "since").and_then(|s| s.trim().parse::<u64>().ok());
    // 单次最多返回多少行。界面正常轮询时只会拿到几行，
    // 这个上限是防"界面关了很久再打开"
    let limit: usize = query_param(query, "limit")
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(500)
        .clamp(1, vca_platform::logbuf::MAX_LINES);

    let (lines, total, cleared) = vca_platform::logbuf::snapshot(since, limit);
    let log_file = opts.data_root.join("logs").join("ui.log");

    Ok(json!({
        "ok": true,
        // 文件路径仍然返回：排查问题时用户需要知道原始日志在哪
        "path": log_file.display().to_string(),
        "file_exists": log_file.is_file(),
        "lines": lines.iter().map(|l| l.to_json()).collect::<Vec<_>>(),
        "total_lines": total,
        // 界面据 cleared 判断"你的游标作废了，请从头拉一次"
        "cleared": cleared,
        // 最新游标，界面存下来给下次用
        "cursor": lines.last().map(|l| l.seq),
    }))
}

/// 清空内存里的日志缓冲（界面「清空」按钮）。
///
/// **不清日志文件** —— 用户点清空是想让界面干净，不是想销毁排查记录。
/// 这一点在返回里也写清楚，免得用户以为磁盘上也没了。
fn clear_logs() -> Result<Value> {
    vca_platform::logbuf::clear();
    Ok(json!({
        "ok": true,
        "note": "界面上的日志已清空；磁盘上的 ui.log 保留，供事后排查",
    }))
}

/// 从 URL query 里取一个参数（复用 server 里那份，避免两套解析逻辑漂移）。
fn query_param(query: &str, key: &str) -> Option<String> {
    crate::server::query_param(query, key)
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
    // 时间表走「合并」：导入是**新增**一份，不能把用户已排好的作息顶掉
    // （和 `import::write_current` 保持同一语义，两处别各写一套）。
    let merged_tt = merge_imported_timetable(opts, &imp.timetable);
    for (sub, name, yaml) in [
        (
            "timetable",
            "current.yaml",
            serde_yaml::to_string(&merged_tt)?,
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

/// 把导入得到的时间表合并进现有列表：同 id 替换，其余保留，新的设为启用。
fn merge_imported_timetable(
    opts: &ServeOptions,
    imported: &vca_core::model::Timetable,
) -> vca_core::config::TimetableFile {
    let mut list = load_timetables(opts);
    list.retain(|t| t.id != imported.id);
    for t in list.iter_mut() {
        t.is_active = false;
    }
    let mut added = imported.clone();
    added.is_active = true;
    list.push(added);
    vca_core::config::TimetableFile { timetables: list }
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
    const PORTS: &[u16] = &[3000, 3001, 5700, 6099, 8080, 12345];
    let mut found = Vec::new();
    let mut onebot = Vec::new();

    for p in PORTS {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], *p));
        if std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300))
            .is_ok()
        {
            found.push(*p);
            // 「端口上有人听」和「这是个 OneBot HTTP 服务」是两回事。
            // 只报前者会害人：NapCat 的 WebUI、随便一个本地服务都会让端口亮起来，
            // 用户照着填进去，发消息时得到一句 404 —— 就是本次踩的坑。
            // 所以这里真去问一句 OneBot 的标准动作 get_login_info。
            if let Some(info) = probe_onebot(*p) {
                onebot.push(json!({"port": p, "info": info}));
            }
        }
    }

    Ok(json!({
        "ok": true,
        "found": found,
        "onebot": onebot,
        "candidates": PORTS,
        "napcat_url": vca_platform::napcat::DOWNLOAD_PAGE,
    }))
}

/// 问一句 `get_login_info`，判断这个端口上是不是 OneBot HTTP 服务。
///
/// 返回 `Some("昵称(QQ号)")` 表示确认是；`None` 表示不是（或没开 HTTP 服务）。
fn probe_onebot(port: u16) -> Option<String> {
    let url = format!("http://127.0.0.1:{port}/get_login_info");
    let resp = vca_platform::http::post_json(&url, "{}", 1500).ok()?;
    if !resp.is_success() {
        return None;
    }
    let v: Value = serde_json::from_str(&resp.body).ok()?;
    // OneBot 一定回 retcode；只认这个，避免把别家 JSON 接口误判成 OneBot
    v.get("retcode")?;
    let data = v.get("data").cloned().unwrap_or(Value::Null);
    let nick = data.get("nickname").and_then(|x| x.as_str()).unwrap_or("");
    let uid = data
        .get("user_id")
        .map(|x| x.to_string())
        .unwrap_or_default();
    let uid = uid.trim_matches('"').to_string();
    Some(if nick.is_empty() && uid.is_empty() {
        "已登录".to_string()
    } else {
        format!("{nick}({uid})")
    })
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

    // NapCat 的 OneBot 配置：HTTP 服务端开没开，就在这个文件里。
    // 用户踩过的 404（Cannot POST /send_private_msg）根因就是它是空的。
    let cfg_path = vca_platform::napcat::find_onebot_config(&root);
    let cfg_val = cfg_path
        .as_ref()
        .and_then(|p| vca_platform::napcat::read_json(p));
    let listening = cfg_val
        .as_ref()
        .and_then(vca_platform::napcat::http_server_port);

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
        // 汇总所有日志来源（我们的 launcher log + NapCat 自己的）
        "log": vca_platform::napcat::tail_logs(&root, 6000),
        "onebot_config": cfg_path.as_ref().map(|p| p.display().to_string()),
        "onebot_qq": cfg_path.as_deref().and_then(vca_platform::napcat::qq_from_config_path),
        // 配置里正在听的 HTTP 端口；None = 没开 HTTP 服务端（这正是发消息 404 的原因）
        "onebot_http_port": listening,
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

/// 一键配置：替用户往 NapCat 的 OneBot 配置里写一个开着的 HTTP 服务端。
///
/// 这是「点一下就能用」的关键一步：NapCat 默认**不开** HTTP 服务端，
/// 而界面上填的那个「服务地址」只有在它开了之后才有效。
/// 光让用户去 WebUI 里找，正是我们想省掉的事。
fn bot_configure(opts: &ServeOptions, body: &str) -> Result<Value> {
    let v: Value = serde_json::from_str(body).unwrap_or_else(|_| json!({}));
    let port = v
        .get("port")
        .and_then(|x| x.as_u64())
        .filter(|p| *p > 0 && *p < 65536)
        .unwrap_or(3000) as u16;
    let token = v
        .get("token")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    let root = vca_platform::napcat::root();
    match vca_platform::napcat::ensure_http_server(&root, port, &token) {
        Ok((path, changed)) => {
            // 令牌同时写进 secrets，供推送时使用（推送读的就是 VCA_PUSH_TOKEN）
            if !token.is_empty() {
                let sp = vca_core::secrets::default_path(&opts.config_root);
                let _ = vca_core::secrets::upsert(&sp, "VCA_PUSH_TOKEN", &token);
                vca_core::secrets::set_override("VCA_PUSH_TOKEN", &token);
            }
            Ok(json!({
                "ok": true,
                "path": path.display().to_string(),
                "changed": changed,
                "qq": vca_platform::napcat::qq_from_config_path(&path),
                "port": port,
                "message": if changed {
                    format!("已写入配置（端口 {port}）。重启 NapCat 后生效。")
                } else {
                    format!("配置本来就是这样（端口 {port}），没动它。")
                },
            }))
        }
        Err(e) => Ok(json!({"ok": false, "error": e.to_string()})),
    }
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
        // 摘掉托盘图标再走：直接 exit 会把图标留在任务栏上（shell 不会因为
        // 进程消失就立刻收回它），表现就是「退出了但图标还在、点它没反应」。
        // 两个出口（这里的 /api/quit 和托盘菜单的「退出」）必须做同一件事。
        crate::server::release_tray();
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

// ---------------------------------------------------------------- 依赖补齐

/// 依赖补齐的运行状态（进程级单例）。
///
/// 与 NapCat 安装那套（[`install_state`]）同构：后台线程干活，
/// 界面轮询查进度。刻意**不共用**同一份状态 —— 两者可能同时进行
/// （用户一边装机器人一边补 ffmpeg），共用一个 "running" 会让它们互相挡住。
#[derive(Default)]
struct DepsFetchState {
    /// 是否正在补齐。
    running: bool,
    /// 进度行（给界面显示的最后一条）。
    message: String,
    /// 失败原因。
    error: Option<String>,
}

fn deps_state() -> &'static std::sync::Mutex<DepsFetchState> {
    static S: std::sync::OnceLock<std::sync::Mutex<DepsFetchState>> = std::sync::OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(DepsFetchState::default()))
}

/// 当前是否正在补齐。供启动检查判断要不要弹窗。
pub fn deps_fetching() -> bool {
    deps_state().lock().map(|s| s.running).unwrap_or(false)
}

/// `tools/` 目录：以 exe 所在目录为基准，与既有的 ffmpeg / whisper 查找规则一致。
fn tools_root() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    vca_platform::deps::tools_root(&exe_dir)
}

/// 检查结果 → JSON（给界面用）。
fn deps_status(opts: &ServeOptions) -> Value {
    let root = tools_root();
    let states = vca_platform::deps::check_all(&root);

    // 「可选项是否已经提醒过」决定这次要不要弹窗。
    let notified = vca_platform::fetch::notify_state::optional_notified(&opts.config_root);
    let need = vca_platform::deps::need_notice(&states, notified);

    let items: Vec<Value> = states
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "label": s.label,
                "consequence": s.consequence,
                "required": s.required,
                "ready": s.ready,
                "reason": s.reason,
                "path": s.path.to_string_lossy(),
            })
        })
        .collect();

    json!({
        "ok": true,
        "tools_root": root.to_string_lossy(),
        "items": items,
        // missing 是「全部缺失项」（界面用来展示清单）；
        // notice 是「这次该提醒的」（按严重程度过滤过的）。
        "missing": items.iter().filter(|i| i["ready"] == json!(false)).count(),
        "need_notice": need.len(),
        "notice_required": need.iter().any(|s| s.required),
        "all_auto_fixable": vca_platform::deps::all_auto_fixable(&need),
        "fetching": deps_fetching(),
    })
}

/// 记下「可选项已经提醒过」，此后不再为可选项弹窗。
fn deps_dismiss(opts: &ServeOptions, body: &str) -> Value {
    let v: Value = serde_json::from_str(body).unwrap_or_else(|_| json!({}));
    // 默认就把可选项标记为已提醒；显式传 all=true 时同样处理
    // （当前没有"永久忽略必需项"的语义，必需项永远会提醒）。
    let _ = v;
    vca_platform::fetch::notify_state::mark_optional_notified(&opts.config_root);
    tracing::info!("依赖提示：可选项标记为已提醒，后续不再重复弹窗");
    json!({"ok": true})
}

/// 查询补齐进度。
fn deps_fetch_json() -> Value {
    match deps_state().lock() {
        Ok(s) => json!({
            "running": s.running,
            "message": s.message,
            "error": s.error,
        }),
        Err(_) => json!({"running": false, "message": "", "error": "状态不可读"}),
    }
}

/// 开始补齐缺失的依赖。
fn deps_fetch(opts: &ServeOptions, body: &str) -> Result<Value> {
    {
        let s = deps_state()
            .lock()
            .map_err(|_| anyhow::anyhow!("状态锁失效"))?;
        if s.running {
            return Ok(json!({"ok": false, "error": "上一次补齐还没结束，等它跑完再试"}));
        }
    }

    // only：只补指定的 id。留空表示补齐全部缺失项。
    let v: Value = serde_json::from_str(body).unwrap_or_else(|_| json!({}));
    let only: Option<Vec<String>> = v.get("only").and_then(|x| x.as_array()).map(|a| {
        a.iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect()
    });

    let root = tools_root();
    let config_root = opts.config_root.clone();
    // 线程里要用，先读出来（`opts` 不能整个 move 进去，还要留给别处）。
    let mirrors = load_download_settings(opts);

    if let Ok(mut s) = deps_state().lock() {
        s.running = true;
        s.message = "正在检查…".to_string();
        s.error = None;
    }

    tracing::info!("开始补齐依赖（目录 {}）", root.display());

    std::thread::spawn(move || {
        // 进度回调：把事件翻成人话写进状态，界面轮询就能看到。
        // 用 move 捕获一个克隆出来的 mpsc？不需要 —— 直接闭包里更新全局状态，
        // 因为状态本身是进程级单例。
        let progress: vca_platform::fetch::ProgressFn = Box::new(move |e| {
            use vca_platform::fetch::FetchEvent as E;
            // 先把「这次是不是失败、失败原因是什么」摘出来，再构造文案。
            // 不能先 move 掉 `e` 的字段再回头借用 `e` —— 那样是借用已移出的值。
            let fail = match &e {
                E::Failed { error, .. } => Some(error.clone()),
                _ => None,
            };
            let msg = match e {
                E::Start { label, .. } => format!("开始补齐 {label}"),
                E::Trying {
                    url, index, total, ..
                } => {
                    format!("下载中（源 {index}/{total}）{url}")
                }
                E::Downloaded { bytes, .. } => {
                    format!("已下载 {} MB，正在处理…", bytes / 1048576)
                }
                E::Extracting { .. } => "正在解压…".to_string(),
                E::Done { path, bytes, .. } => {
                    format!("完成：{}（{} MB）", path.display(), bytes / 1048576)
                }
                E::Failed { id, error } => format!("{id} 补齐失败：{error}"),
                E::Finished { ok, failed } => {
                    format!("补齐结束：成功 {ok} 项，失败 {failed} 项")
                }
            };
            tracing::info!("依赖补齐：{msg}");
            if let Ok(mut s) = deps_state().lock() {
                s.message = msg;
                if let Some(error) = fail {
                    s.error = Some(error);
                }
            }
        });

        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        // 用带镜像的版本：用户填的源排在最前面，内置地址保底。
        let result = vca_platform::fetch::fetch_missing_with_mirrors(
            &root,
            only.as_deref(),
            Some(progress),
            Some(cancel),
            &mirrors,
        );

        // 补完再看一遍：如果可选项现在齐了，顺手把"已提醒"标记写上，
        // 免得下次启动因为残留的失败项又弹一次。
        let all_ok = result.iter().filter(|s| !s.required).all(|s| s.ready);
        if all_ok {
            vca_platform::fetch::notify_state::mark_optional_notified(&config_root);
        }

        let bad = result.iter().filter(|s| !s.ready).count();
        if let Ok(mut s) = deps_state().lock() {
            s.running = false;
            if bad == 0 {
                s.message = "全部依赖已就绪".to_string();
                s.error = None;
            } else {
                s.message = format!("仍有 {bad} 项缺失");
            }
        }
        tracing::info!("依赖补齐结束，仍有 {bad} 项缺失");
    });

    Ok(json!({"ok": true, "message": "已开始补齐，进度会显示在这里"}))
}

// ============================================================================
// 下载源（镜像）自定义
// ============================================================================

/// 读出用户配置的下载源设置。
///
/// 读不到就返回默认值（=不用自定义源）。**不报错** ——
/// 配置文件坏了不该让"下载"这个功能整个不可用，
/// 退化成"只用内置源"是个合理的降级。
fn load_download_settings(opts: &ServeOptions) -> vca_core::config::DownloadSettings {
    let path = settings_path(opts);
    match std::fs::read_to_string(&path) {
        Ok(text) => match serde_yaml::from_str::<vca_core::config::Settings>(&text) {
            Ok(s) => s.download,
            Err(e) => {
                tracing::warn!("读下载源配置失败（用内置源）：{e}");
                Default::default()
            }
        },
        Err(_) => Default::default(),
    }
}

/// 把下载源设置写回去。
///
/// 走"读整个 settings.yaml → 只改 download 字段 → 写回"，
/// 而不是重新拼一个文件：**用户的其他配置必须原样保留**。
/// 用 `serde_yaml::Value` 中转是为了不因为 `Settings` 结构里
/// 将来加字段而把用户已有的配置吃掉。
fn save_download_settings(
    opts: &ServeOptions,
    d: &vca_core::config::DownloadSettings,
) -> Result<()> {
    let path = settings_path(opts);
    let mut root: serde_yaml::Value = match std::fs::read_to_string(&path) {
        Ok(text) => serde_yaml::from_str(&text).map_err(|e| {
            anyhow::anyhow!(
                "现有配置 {} 格式有误，为避免覆盖其它设置，未保存下载源：{e}",
                path.display()
            )
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_yaml::Value::Null,
        Err(e) => {
            return Err(anyhow::anyhow!("读取 {} 失败：{e}", path.display()));
        }
    };
    if !root.is_mapping() {
        root = serde_yaml::Value::Mapping(Default::default());
    }
    let dl = serde_yaml::to_value(d).map_err(|e| anyhow::anyhow!("序列化下载源失败：{e}"))?;
    root.as_mapping_mut()
        .expect("上面刚确认过是 mapping")
        .insert(serde_yaml::Value::from("download"), dl);

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let text = serde_yaml::to_string(&root).map_err(|e| anyhow::anyhow!("写下载源失败：{e}"))?;
    std::fs::write(&path, text).map_err(|e| anyhow::anyhow!("写 {} 失败：{e}", path.display()))?;
    Ok(())
}

/// GET /api/deps/mirrors —— 读当前下载源配置。
///
/// 同时把**内置源**一并返回：界面要让用户看到"不填会用什么"，
/// 否则他没法判断自己该不该填。这也是"透明"的一部分 ——
/// 用户有权知道程序会去哪些地址下东西。
fn deps_mirrors_get(opts: &ServeOptions) -> Value {
    let d = load_download_settings(opts);
    json!({
        "ok": true,
        "prefix": d.prefix,
        "mirrors": d.mirrors,
        "only_custom": d.only_custom,
        // 内置源：只读，给界面展示用
        "builtin": builtin_sources(),
        // 依赖清单（含名称/是否必需），界面拿它渲染表单
        "deps": vca_platform::deps::DEPS.iter().map(|s| json!({
            "id": s.id,
            "label": s.label,
            "required": s.required,
            "consequence": s.consequence,
        })).collect::<Vec<_>>(),
    })
}

/// 内置的下载源（只读展示）。
fn builtin_sources() -> Value {
    let mut m = serde_json::Map::new();
    for (id, urls) in vca_platform::fetch::builtin_urls() {
        m.insert(id.to_string(), json!(urls));
    }
    Value::Object(m)
}

/// POST /api/deps/mirrors —— 保存下载源配置。
///
/// 保存前**逐条校验**：填错的地址要让用户当场知道，
/// 而不是等到下载失败才猜"是我填错了还是网络问题"。
fn deps_mirrors_set(opts: &ServeOptions, body: &str) -> Result<Value> {
    let v: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => {
            return Ok(json!({
                "ok": false,
                "error": format!("请求 JSON 格式不合法：{e}"),
                "field": "body",
            }));
        }
    };
    if !v.is_object() {
        return Ok(json!({
            "ok": false,
            "error": "请求体必须是 JSON 对象",
            "field": "body",
        }));
    }

    let mut d = vca_core::config::DownloadSettings::default();

    // prefix：缺省/空数组 = 不设置全局前缀。带不带结尾斜杠都接受，
    // 保存时统一规范成恰好一个斜杠，避免拼接歧义。
    if let Some(raw) = v.get("prefix") {
        let Some(arr) = raw.as_array() else {
            return Ok(json!({
                "ok": false,
                "error": "prefix 必须是字符串数组",
                "field": "prefix",
            }));
        };
        for item in arr {
            let Some(s) = item.as_str() else {
                return Ok(json!({
                    "ok": false,
                    "error": "prefix 数组里的每一项都必须是字符串",
                    "field": "prefix",
                }));
            };
            let s = s.trim();
            if s.is_empty() {
                continue;
            }
            if let Err(e) = vca_core::config::validate_mirror_prefix(s) {
                return Ok(json!({
                    "ok": false,
                    "error": format!("前缀镜像「{s}」不合法：{e}"),
                    "field": "prefix",
                }));
            }
            let normalized = vca_core::config::normalize_mirror_prefix(s);
            if !d.prefix.contains(&normalized) {
                d.prefix.push(normalized);
            }
        }
    }

    // mirrors: { depId: [url, ...] }。
    // 未知 id 必须报错而不是静默保存：否则 UI 看起来“保存成功”，真实下载器
    // 却永远不会读取那个键，这是最难发现的一类假成功。
    if let Some(raw) = v.get("mirrors") {
        let Some(obj) = raw.as_object() else {
            return Ok(json!({
                "ok": false,
                "error": "mirrors 必须是对象",
                "field": "mirrors",
            }));
        };
        for (k, arr) in obj {
            if !vca_platform::deps::DEPS.iter().any(|dep| dep.id == k) {
                return Ok(json!({
                    "ok": false,
                    "error": format!("未知的依赖 id：{k}"),
                    "field": k,
                }));
            }
            let Some(list) = arr.as_array() else {
                return Ok(json!({
                    "ok": false,
                    "error": format!("{k} 的下载地址必须是字符串数组"),
                    "field": k,
                }));
            };
            let mut urls = Vec::new();
            for item in list {
                let Some(s) = item.as_str() else {
                    return Ok(json!({
                        "ok": false,
                        "error": format!("{k} 的每个下载地址都必须是字符串"),
                        "field": k,
                    }));
                };
                let s = s.trim();
                if s.is_empty() {
                    continue;
                }
                if let Err(e) = vca_core::config::validate_mirror_url(s) {
                    return Ok(json!({
                        "ok": false,
                        "error": format!("{k} 的地址「{s}」不合法：{e}"),
                        "field": k,
                    }));
                }
                let s = s.to_string();
                if !urls.contains(&s) {
                    urls.push(s);
                }
            }
            if !urls.is_empty() {
                d.mirrors.insert(k.clone(), urls);
            }
        }
    }

    // only_custom：缺省 = false；传了就必须真的是 bool，不能把字符串 "true"
    // 静默当 false（那会违背用户“只走内网源”的明确选择）。
    if let Some(raw) = v.get("only_custom") {
        let Some(b) = raw.as_bool() else {
            return Ok(json!({
                "ok": false,
                "error": "only_custom 必须是布尔值",
                "field": "only_custom",
            }));
        };
        d.only_custom = b;
    }

    // 开了"只用自定义源"却一个源都没填 —— 那是**必然下不动**的配置，
    // 必须拦住。否则用户会得到一个"点了没反应"的程序。
    if d.only_custom && d.is_empty() {
        return Ok(json!({
            "ok": false,
            "error": "勾了「只用自定义源」但一个地址都没填，这样必然下载失败。\
                      请取消勾选，或至少填一个地址。",
            "field": "only_custom",
        }));
    }

    save_download_settings(opts, &d)?;
    tracing::info!(
        "下载源已更新：prefix={:?} mirrors={} only_custom={}",
        d.prefix,
        d.mirrors.len(),
        d.only_custom
    );

    // 回读一遍返回"真正存下去的东西"：写盘可能和内存里的不同
    // （比如 YAML 的空值处理），让界面显示实际生效的配置。
    let saved = load_download_settings(opts);
    Ok(json!({
        "ok": true,
        "message": "下载源已保存",
        "prefix": saved.prefix,
        "mirrors": saved.mirrors,
        "only_custom": saved.only_custom,
    }))
}

/// POST /api/deps/mirrors/test —— 测一个地址能不能连上。
///
/// # 为什么要这个
///
/// 用户填了地址之后最需要知道的就是"它到底通不通"。没有这个，
/// 他只能去点下载、等失败、再猜原因 —— 而下载失败要等超时，很慢，
/// 而且失败原因混着"地址错"和"网络差"两种可能，说不清。
///
/// 这里只做**轻量探测**：发 HEAD（拿不到就退化成极小范围 GET），
/// 只看能不能连上、HTTP 状态是什么。**不下载整个文件** ——
/// 那要好几十 MB，测一次等半天就没人用了。
fn deps_mirrors_test(body: &str) -> Result<Value> {
    let v: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => {
            return Ok(json!({
                "ok": false,
                "error": format!("请求 JSON 格式不合法：{e}"),
                "field": "body",
            }));
        }
    };
    let Some(url) = v.get("url").and_then(|x| x.as_str()) else {
        return Ok(json!({"ok": false, "error": "没给地址", "field": "url"}));
    };
    let url = url.trim();

    if let Err(e) = vca_core::config::validate_mirror_url(url) {
        return Ok(json!({
            "ok": false,
            "error": format!("地址不合法：{e}"),
            "field": "url",
        }));
    }

    match vca_platform::http::probe(url, 8000) {
        Ok(info) => Ok(json!({
            "ok": true,
            "status": info.status,
            "kind": info.kind,
            "size": info.size,
            "message": info.describe(),
        })),
        Err(e) => Ok(json!({
            "ok": false,
            "error": format!("连不上：{e}"),
            "field": "url",
            // 给一句人话建议，别让用户对着错误码发呆
            "hint": "确认地址能在浏览器里直接下载文件（不是网页）。\
                     如果是内网镜像，确认这台机器能访问它。",
        })),
    }
}

#[cfg(test)]
mod mirror_tests {
    use super::*;

    fn opts(tag: &str) -> ServeOptions {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "vca-mirror-api-{tag}-{}-{stamp}",
            std::process::id()
        ));
        ServeOptions {
            config_root: root.join("config"),
            data_root: root.join("data"),
            profile: "teacher-a".to_string(),
            open_browser: false,
            port: 0,
            web_dir: None,
        }
    }

    fn cleanup(opts: &ServeOptions) {
        if let Some(root) = opts.config_root.parent() {
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn empty_config_uses_confirmed_builtin_defaults() {
        let o = opts("defaults");
        let got = deps_mirrors_get(&o);
        assert_eq!(got["ok"], true);
        assert_eq!(got["prefix"], json!([]));
        assert_eq!(got["mirrors"], json!({}));
        assert_eq!(got["only_custom"], false);
        for id in ["ffmpeg", "whisper", "model"] {
            assert!(
                got["builtin"][id].as_array().is_some_and(|a| !a.is_empty()),
                "{id} 没有默认公开源：{got}"
            );
        }
        cleanup(&o);
    }

    #[test]
    fn save_normalizes_persists_and_applies_without_restart() {
        let o = opts("save");
        let path = settings_path(&o);
        std::fs::create_dir_all(path.parent().expect("配置父目录")).unwrap();
        // 模拟未来版本/插件写入的未知字段：保存 download 时不能把它吃掉。
        std::fs::write(&path, "profile: teacher-a\nfuture_field: keep-me\n").unwrap();

        let first = deps_mirrors_set(
            &o,
            r#"{
                "prefix": [" https://mirror.example/// ", "https://mirror.example/"],
                "mirrors": {
                    "ffmpeg": [" https://files.example/ffmpeg.zip ", "", "https://files.example/ffmpeg.zip"]
                },
                "only_custom": false
            }"#,
        )
        .expect("保存成功");
        assert_eq!(first["ok"], true, "{first}");
        assert_eq!(first["prefix"], json!(["https://mirror.example/"]));
        assert_eq!(
            first["mirrors"]["ffmpeg"],
            json!(["https://files.example/ffmpeg.zip"])
        );

        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(yaml["future_field"].as_str(), Some("keep-me"));

        // 同一进程内再改一次并立刻回读，证明配置不需要重新编译/重启。
        let second = deps_mirrors_set(
            &o,
            r#"{
                "prefix": [],
                "mirrors": {"model": ["https://files.example/tiny.bin"]},
                "only_custom": true
            }"#,
        )
        .expect("第二次保存成功");
        assert_eq!(second["ok"], true, "{second}");
        let live = load_download_settings(&o);
        assert!(live.prefix.is_empty());
        assert!(live.only_custom);
        assert_eq!(
            live.custom_for("model"),
            &["https://files.example/tiny.bin".to_string()]
        );

        cleanup(&o);
    }

    #[test]
    fn malformed_existing_yaml_is_never_overwritten() {
        let o = opts("bad-yaml");
        let path = settings_path(&o);
        std::fs::create_dir_all(path.parent().expect("配置父目录")).unwrap();
        let broken = "profile: [这不是完整 YAML\n";
        std::fs::write(&path, broken).unwrap();

        let result = deps_mirrors_set(&o, r#"{"prefix":[],"mirrors":{},"only_custom":false}"#);
        assert!(result.is_err(), "损坏配置上不应继续覆盖：{result:?}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            broken,
            "损坏配置被覆盖，用户其它设置会丢失"
        );
        cleanup(&o);
    }

    #[test]
    fn invalid_shapes_urls_and_unknown_ids_are_rejected() {
        let o = opts("invalid");
        let cases = [
            ("{", "body"),
            (r#"{"prefix":"https://x/"}"#, "prefix"),
            (r#"{"prefix":["https://x/?url="]}"#, "prefix"),
            (r#"{"mirrors":[]}"#, "mirrors"),
            (r#"{"mirrors":{"unknown":["https://x/a"]}}"#, "unknown"),
            (r#"{"mirrors":{"ffmpeg":["ftp://x/a.zip"]}}"#, "ffmpeg"),
            (r#"{"mirrors":{"ffmpeg":[123]}}"#, "ffmpeg"),
            (r#"{"only_custom":"true"}"#, "only_custom"),
            (
                r#"{"prefix":[],"mirrors":{},"only_custom":true}"#,
                "only_custom",
            ),
        ];
        for (body, field) in cases {
            let got = deps_mirrors_set(&o, body).expect("接口正常返回");
            assert_eq!(got["ok"], false, "非法输入被放过：{body} -> {got}");
            assert_eq!(got["field"], field, "字段定位不对：{body} -> {got}");
        }
        cleanup(&o);
    }
}
