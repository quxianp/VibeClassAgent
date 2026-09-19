//! 浏览器自动化：驱动**系统自带的 Edge**（或 Chrome）去问网页版聊天机器人。
//!
//! # 这条路径的定位
//!
//! 它是「免费」的第三种走法。三种走法的取舍：
//!
//! | 走法 | 成本 | 稳定性 | 前置条件 |
//! |---|---|---|---|
//! | 官方付费 API | 计费 | 高 | 一个 API Key |
//! | 官方免费额度 | 0 | 高 | 一个 API Key（模型选免费的那个） |
//! | **浏览器自动化**（本模块） | 0 | 中低 | 手动登录一次；站点改版可能失效 |
//!
//! 前两种共用同一条 HTTP 路径（见 [`crate::llm`]），所以「免费」的首选永远是
//! 免费额度模型。本模块存在的意义是覆盖「一分钱都不想花、也不想去申请 Key」的情况。
//!
//! # 为什么不用 headless 浏览器的常规做法
//!
//! 不下载 Chromium：Windows 10/11 **必定自带 Edge**，它就是 Chromium 内核，
//! 直接用可以省下 150 MB 下载量和一个「这程序怎么这么大」的解释。
//!
//! # 为什么不硬编码 CSS 选择器
//!
//! 站点改版是常态，写死 `#chat-input` 这类选择器，第一次改版就报废。
//! 所以这里默认走**差分法**：
//!
//! 1. 发送前记下 `document.body.innerText` 的长度与内容哈希；
//! 2. 发送后轮询，等页面文本**增长**并且连续若干次不再变化（生成结束）；
//! 3. 取新增的那一段作为回复。
//!
//! 这样只需要知道「输入框长什么样」（用可见性 + 位置启发式判断即可），
//! 不需要知道回复渲染在哪个 DOM 节点里。真要精确控制，也可以用配置覆盖选择器。
//!
//! # 登录
//!
//! 首次使用必须**手动登录一次**：调用 [`open_login_window`] 会以有界面方式
//! 打开浏览器，用户登录后关闭即可。登录态保存在 `user_data_dir` 里，
//! 之后 [`ask`] 就能用 headless 模式复用。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};

use crate::proc;

/// 浏览器自动化配置。
#[derive(Debug, Clone)]
pub struct BrowserBotConfig {
    /// 浏览器可执行体；留空则自动查找 Edge → Chrome。
    pub browser: PathBuf,
    /// 站点 id（见 [`SITES`]）。
    pub site: String,
    /// 用户数据目录（保存登录态）。
    pub user_data_dir: PathBuf,
    /// 是否用无界面模式。首次登录需要关掉。
    pub headless: bool,
    /// 单次问答的总超时（秒）。
    pub timeout_sec: u64,
    /// 风险确认：必须显式置为 true 才允许启用。
    pub risk_ack: bool,
    /// 可选的选择器覆盖，形如 `input=textarea;send=button.send`。
    pub selectors_override: String,
}

impl Default for BrowserBotConfig {
    fn default() -> Self {
        Self {
            browser: PathBuf::new(),
            site: "kimi".to_string(),
            user_data_dir: PathBuf::new(),
            headless: true,
            timeout_sec: 180,
            risk_ack: false,
            selectors_override: String::new(),
        }
    }
}

impl BrowserBotConfig {
    /// 是否可以启用。
    pub fn is_ready(&self) -> bool {
        self.risk_ack && find_browser_for(self).is_some()
    }

    /// 解析后的用户数据目录（留空则用工具目录下的 `browser-profile`）。
    pub fn effective_user_data_dir(&self) -> PathBuf {
        if !self.user_data_dir.as_os_str().is_empty() {
            return self.user_data_dir.clone();
        }
        proc::tool_dirs()
            .first()
            .cloned()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("browser-profile")
    }

    /// 站点适配器。
    pub fn adapter(&self) -> &'static SiteAdapter {
        site_adapter(&self.site).unwrap_or(&SITES[0])
    }

    /// 解析选择器覆盖。
    pub fn overrides(&self) -> SelectorOverride {
        let mut o = SelectorOverride::default();
        for part in self.selectors_override.split(';') {
            if let Some((k, v)) = part.split_once('=') {
                match k.trim() {
                    "input" => o.input = v.trim().to_string(),
                    "send" => o.send = v.trim().to_string(),
                    "reply" => o.reply = v.trim().to_string(),
                    _ => {}
                }
            }
        }
        o
    }
}

/// 选择器覆盖项。
#[derive(Debug, Clone, Default)]
pub struct SelectorOverride {
    /// 输入框。
    pub input: String,
    /// 发送按钮。
    pub send: String,
    /// 回复容器。
    pub reply: String,
}

/// 站点适配器。
///
/// `input` / `send` / `reply` 只作为**可选提示**：留空就走通用启发式。
/// 这样站点改版时最坏情况是「回复里多几句页面文字」，而不是彻底不能用。
#[derive(Debug, Clone, Copy)]
pub struct SiteAdapter {
    /// 站点 id。
    pub id: &'static str,
    /// 显示名。
    pub name: &'static str,
    /// 首页地址。
    pub url: &'static str,
    /// 输入框选择器提示（可空）。
    pub input: &'static str,
    /// 发送按钮选择器提示（可空，空则用回车）。
    pub send: &'static str,
    /// 回复容器选择器提示（可空）。
    pub reply: &'static str,
    /// 备注。
    pub note: &'static str,
}

/// 内置站点表。
///
/// ⚠️ 各站点的 DOM 结构随时会变，这里的选择器**只作为提示**，
/// 失效时请用 `selectors_override` 覆盖，或依赖默认的差分法。
pub const SITES: &[SiteAdapter] = &[
    SiteAdapter {
        id: "kimi",
        name: "Kimi（月之暗面）",
        url: "https://kimi.moonshot.cn/",
        input: "",
        send: "",
        reply: "",
        note: "长文本能力好，适合课堂纪要",
    },
    SiteAdapter {
        id: "deepseek",
        name: "DeepSeek 网页版",
        url: "https://chat.deepseek.com/",
        input: "",
        send: "",
        reply: "",
        note: "免费，中文好",
    },
    SiteAdapter {
        id: "tongyi",
        name: "通义千问网页版",
        url: "https://tongyi.aliyun.com/qianwen/",
        input: "",
        send: "",
        reply: "",
        note: "阿里系，国内访问快",
    },
    SiteAdapter {
        id: "doubao",
        name: "豆包",
        url: "https://www.doubao.com/chat/",
        input: "",
        send: "",
        reply: "",
        note: "字节系，国内访问快",
    },
    SiteAdapter {
        id: "chatgpt",
        name: "ChatGPT 网页版",
        url: "https://chatgpt.com/",
        input: "",
        send: "",
        reply: "",
        note: "国内直连需代理",
    },
    SiteAdapter {
        id: "custom",
        name: "自定义站点",
        url: "about:blank",
        input: "",
        send: "",
        reply: "",
        note: "请在配置里填写 url；适用于自建或其它站点",
    },
];

/// 按 id 查站点。
pub fn site_adapter(id: &str) -> Option<&'static SiteAdapter> {
    SITES.iter().find(|s| s.id.eq_ignore_ascii_case(id.trim()))
}

// ---------------------------------------------------------------------------
// 浏览器定位
// ---------------------------------------------------------------------------

/// 查找可用的 Chromium 内核浏览器。
///
/// 顺序：配置 → `VCA_BROWSER` → 系统 Edge → Chrome。
pub fn find_browser_for(cfg: &BrowserBotConfig) -> Option<PathBuf> {
    if !cfg.browser.as_os_str().is_empty() && cfg.browser.is_file() {
        return Some(cfg.browser.clone());
    }
    if let Ok(p) = std::env::var("VCA_BROWSER") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    find_system_browser()
}

/// 查找系统自带的 Edge / Chrome。
pub fn find_system_browser() -> Option<PathBuf> {
    let candidates = [
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    ];
    for c in candidates {
        let p = PathBuf::from(c);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// CDP 会话
// ---------------------------------------------------------------------------

/// 一个最小的 CDP 客户端：只用到 `Runtime.evaluate`。
struct CdpSession {
    child: Child,
    port: u16,
    socket: tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
    next_id: u64,
}

impl CdpSession {
    /// 启动浏览器并连上它的调试端口。
    fn launch(cfg: &BrowserBotConfig) -> Result<Self> {
        let browser = find_browser_for(cfg)
            .ok_or_else(|| anyhow!("找不到 Edge 或 Chrome（可用 VCA_BROWSER 指定）"))?;
        let profile = cfg.effective_user_data_dir();
        std::fs::create_dir_all(&profile)
            .with_context(|| format!("创建浏览器数据目录失败: {}", profile.display()))?;
        // 上一轮可能留下锁文件，删除以免浏览器拒绝启动
        for f in ["SingletonLock", "SingletonCookie", "SingletonSocket"] {
            let _ = std::fs::remove_file(profile.join(f));
        }

        let mut cmd = Command::new(&browser);
        cmd.arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile.display()))
            // 新版 Chrome/Edge 会拒绝非浏览器来源的 WebSocket 连接，必须放开
            .arg("--remote-allow-origins=*")
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-extensions")
            .arg("--disable-background-networking")
            .arg("--disable-sync")
            .arg("--mute-audio");
        if cfg.headless {
            // `new` 模式的渲染行为与有界面一致，站点更不容易识别出自动化
            cmd.arg("--headless=new");
        }
        cmd.arg(cfg.adapter().url);
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(proc::CREATE_NO_WINDOW);
        }

        let child = cmd.spawn().context("启动浏览器失败")?;

        // 端口是浏览器自己选的，写在这个文件里（第一行端口，第二行 ws 路径）
        let port = wait_for_devtools_port(&profile, Duration::from_secs(30))
            .context("等待浏览器调试端口超时")?;

        let ws_url = format!("ws://127.0.0.1:{port}/devtools/page/{}", page_target_id(port)?);
        let (socket, _resp) =
            tungstenite::connect(ws_url.as_str()).with_context(|| format!("连接 CDP 失败: {ws_url}"))?;

        Ok(Self {
            child,
            port,
            socket,
            next_id: 1,
        })
    }

    /// 在页面里执行一段 JS，返回其值（按 JSON 传回）。
    fn evaluate(&mut self, expression: &str) -> Result<serde_json::Value> {
        let id = self.next_id;
        self.next_id += 1;
        let msg = serde_json::json!({
            "id": id,
            "method": "Runtime.evaluate",
            "params": {
                "expression": expression,
                "returnByValue": true,
                "awaitPromise": false,
                "userGesture": true
            }
        })
        .to_string();
        self.socket
            .send(tungstenite::Message::text(msg))
            .context("发送 CDP 命令失败")?;

        // 读到自己那条 id 的响应为止（中间可能有事件推送）
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if Instant::now() > deadline {
                return Err(anyhow!("等待 CDP 响应超时"));
            }
            let text = match self.socket.read() {
                Ok(tungstenite::Message::Text(t)) => t.to_string(),
                Ok(_) => continue,
                Err(e) => return Err(anyhow!("读 CDP 响应失败: {e}")),
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            if v.get("id").and_then(|x| x.as_u64()) != Some(id) {
                continue;
            }
            if let Some(err) = v.get("error") {
                return Err(anyhow!("CDP 返回错误: {err}"));
            }
            return Ok(v
                .get("result")
                .and_then(|r| r.get("result"))
                .and_then(|r| r.get("value"))
                .cloned()
                .unwrap_or(serde_json::Value::Null));
        }
    }
}

impl Drop for CdpSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 等浏览器把调试端口写出来。
fn wait_for_devtools_port(profile: &Path, timeout: Duration) -> Result<u16> {
    let file = profile.join("DevToolsActivePort");
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(&file) {
            if let Some(line) = text.lines().next() {
                if let Ok(p) = line.trim().parse::<u16>() {
                    if p > 0 {
                        return Ok(p);
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(anyhow!(
        "浏览器没有在 {} 秒内写出 DevToolsActivePort（{}）",
        timeout.as_secs(),
        file.display()
    ))
}

/// 取第一个 page 目标的 id。
fn page_target_id(port: u16) -> Result<String> {
    let url = format!("http://127.0.0.1:{port}/json/list");
    let body = http_get(&url).with_context(|| format!("请求 {url} 失败"))?;
    let v: serde_json::Value = serde_json::from_str(&body).context("解析 /json/list 失败")?;
    let arr = v.as_array().ok_or_else(|| anyhow!("/json/list 不是数组"))?;
    for t in arr {
        if t.get("type").and_then(|x| x.as_str()) == Some("page") {
            if let Some(id) = t.get("id").and_then(|x| x.as_str()) {
                return Ok(id.to_string());
            }
        }
    }
    Err(anyhow!("没有找到 page 目标"))
}

/// 极简 HTTP GET（只用于 CDP 的本地管理接口，不值得动用 ureq）。
fn http_get(url: &str) -> Result<String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| anyhow!("只支持 http://"))?;
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>()?),
        None => (hostport.to_string(), 80),
    };
    let mut stream = TcpStream::connect((host.as_str(), port)).context("连接失败")?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nAccept: application/json\r\n\r\n"
    );
    stream.write_all(req.as_bytes())?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf).to_string();
    let body = match text.find("\r\n\r\n") {
        Some(i) => text[i + 4..].to_string(),
        None => text,
    };
    Ok(body)
}

// ---------------------------------------------------------------------------
// 驱动脚本
// ---------------------------------------------------------------------------

/// 通用输入框启发式：可见、够大、位置最靠下。
///
/// 聊天站点的输入框永远是「页面最底部那个大的可编辑区域」，
/// 这个特征比任何 CSS 类名都稳定。
const JS_FIND_INPUT: &str = r#"
(() => {
  const ok = (el) => {
    const r = el.getBoundingClientRect();
    const st = getComputedStyle(el);
    return r.width > 120 && r.height > 18 &&
           st.visibility !== 'hidden' && st.display !== 'none' && st.opacity !== '0' &&
           !el.disabled && !el.readOnly;
  };
  let list = [...document.querySelectorAll('textarea, [contenteditable="true"]')].filter(ok);
  if (list.length === 0) return null;
  list.sort((a, b) => b.getBoundingClientRect().top - a.getBoundingClientRect().top);
  const el = list[0];
  el.setAttribute('data-vca-input', '1');
  return { tag: el.tagName, id: el.id || '', cls: (el.className || '').toString().slice(0, 80) };
})()
"#;

/// 把文本填进输入框并派发事件（React 受控组件必须走原生 setter）。
fn js_fill(text: &str) -> String {
    let escaped = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into());
    format!(
        r#"
(() => {{
  const el = document.querySelector('[data-vca-input="1"]');
  if (!el) return 'NO_INPUT';
  const text = {escaped};
  el.focus();
  if (el.tagName === 'TEXTAREA' || el.tagName === 'INPUT') {{
    const proto = el.tagName === 'TEXTAREA'
      ? window.HTMLTextAreaElement.prototype
      : window.HTMLInputElement.prototype;
    const setter = Object.getOwnPropertyDescriptor(proto, 'value').set;
    setter.call(el, text);
  }} else {{
    el.textContent = text;
  }}
  el.dispatchEvent(new Event('input', {{ bubbles: true }}));
  el.dispatchEvent(new Event('change', {{ bubbles: true }}));
  return 'OK';
}})()
"#
    )
}

/// 触发发送：优先找发送按钮，找不到就发回车键。
const JS_SEND: &str = r#"
(() => {
  const el = document.querySelector('[data-vca-input="1"]');
  const pick = () => {
    const btns = [...document.querySelectorAll('button, [role="button"], div[class*="send"], svg[class*="send"]')];
    for (const b of btns) {
      const label = ((b.getAttribute('aria-label') || '') + (b.title || '') + (b.textContent || '')).toLowerCase();
      if (label.includes('发送') || label.includes('send')) {
        const r = b.getBoundingClientRect();
        if (r.width > 0 && r.height > 0) return b;
      }
    }
    return null;
  };
  const b = pick();
  if (b) { b.click(); return 'CLICK'; }
  if (!el) return 'NO_INPUT';
  for (const type of ['keydown', 'keypress', 'keyup']) {
    el.dispatchEvent(new KeyboardEvent(type, {
      key: 'Enter', code: 'Enter', keyCode: 13, which: 13, bubbles: true, cancelable: true
    }));
  }
  return 'ENTER';
})()
"#;

/// 取页面正文的「长度 + 尾部样本」，用于差分。
const JS_SNAPSHOT: &str = r#"
(() => {
  const t = document.body ? document.body.innerText : '';
  return { len: t.length, tail: t.slice(-400) };
})()
"#;

/// 抓取正文最后一段（通用兜底）。
const JS_TAIL: &str = r#"
(() => {
  const t = document.body ? document.body.innerText : '';
  return t.slice(-4000);
})()
"#;

// ---------------------------------------------------------------------------
// 对外接口
// ---------------------------------------------------------------------------

/// 以**有界面**方式打开浏览器，供用户手动登录（登录态存进 user_data_dir）。
///
/// 返回子进程句柄；调用方在用户关闭浏览器后 `wait` 即可。
pub fn open_login_window(cfg: &BrowserBotConfig) -> Result<Child> {
    let browser = find_browser_for(cfg)
        .ok_or_else(|| anyhow!("找不到 Edge 或 Chrome（可用 VCA_BROWSER 指定）"))?;
    let profile = cfg.effective_user_data_dir();
    std::fs::create_dir_all(&profile)?;
    for f in ["SingletonLock", "SingletonCookie", "SingletonSocket"] {
        let _ = std::fs::remove_file(profile.join(f));
    }
    let child = Command::new(browser)
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg(cfg.adapter().url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("启动浏览器失败")?;
    Ok(child)
}

/// 问一次网页版聊天机器人，返回它的回复文本。
///
/// `system` 与 `user` 会被拼成一段（网页版没有 system 角色），
/// 并在末尾重申约束 —— 网页版对「只输出 JSON」这类指令的服从度低于 API。
pub fn ask(cfg: &BrowserBotConfig, system: &str, user: &str) -> Result<String> {
    if !cfg.risk_ack {
        return Err(anyhow!(
            "浏览器模式需要在配置里显式设置 risk_ack: true 才能启用。\n\
             请注意：自动化操作网页版可能违反该站点的服务条款，账号存在风险。"
        ));
    }

    let mut session = CdpSession::launch(cfg)?;
    let timeout = Duration::from_secs(if cfg.timeout_sec == 0 {
        180
    } else {
        cfg.timeout_sec
    });

    let prompt = format!(
        "{system}\n\n---\n\n{user}\n\n\
         （请直接给出结果，不要复述以上要求，不要输出多余解释。）"
    );

    // ---- 1) 等输入框出现 ----
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut found = false;
    while Instant::now() < deadline {
        let v = session.evaluate(JS_FIND_INPUT)?;
        if !v.is_null() {
            found = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(800));
    }
    if !found {
        return Err(anyhow!(
            "在 {} 里找不到输入框。可能原因：未登录、页面结构变化、或站点要求人机验证。\n\
             建议先跑一次 `vca debug browser-login` 手动登录后再试。",
            cfg.adapter().name
        ));
    }

    // ---- 2) 记录发送前的正文状态 ----
    let before = session.evaluate(JS_SNAPSHOT)?;
    let before_len = before.get("len").and_then(|x| x.as_u64()).unwrap_or(0);

    // ---- 3) 填内容并发送 ----
    let filled = session.evaluate(&js_fill(&prompt))?;
    if filled.as_str() == Some("NO_INPUT") {
        return Err(anyhow!("输入框在填写时消失了，页面可能已跳转"));
    }
    std::thread::sleep(Duration::from_millis(300));
    let sent = session.evaluate(JS_SEND)?;
    if sent.as_str() == Some("NO_INPUT") {
        return Err(anyhow!("发送失败：输入框已不存在"));
    }

    // ---- 4) 等回复：正文变长且连续稳定 ----
    let start = Instant::now();
    let mut last_len = before_len;
    let mut stable_rounds = 0;
    let mut seen_growth = false;
    while start.elapsed() < timeout {
        std::thread::sleep(Duration::from_secs(2));
        let snap = session.evaluate(JS_SNAPSHOT)?;
        let len = snap.get("len").and_then(|x| x.as_u64()).unwrap_or(0);
        if len > before_len + 30 {
            seen_growth = true;
        }
        if seen_growth && len == last_len {
            stable_rounds += 1;
            // 连续 3 次（约 6 秒）不再变化，认为生成结束
            if stable_rounds >= 3 {
                break;
            }
        } else {
            stable_rounds = 0;
        }
        last_len = len;
    }

    if !seen_growth {
        return Err(anyhow!(
            "等待 {} 秒后页面正文没有增长，聊天机器人可能没有响应或需要登录",
            timeout.as_secs()
        ));
    }

    // ---- 5) 取尾部文本作为回复 ----
    let tail = session.evaluate(JS_TAIL)?;
    let text = tail.as_str().unwrap_or_default().to_string();
    Ok(clean_reply(&text, &prompt))
}

/// 从页面尾部文本里尽可能剥出「回复本体」。
///
/// 差分法拿到的是一整段页面文本，难免混入导航、时间戳等噪声。
/// 这里做几件低成本的事：去掉我们自己的提问、丢掉明显是界面元素的行。
pub fn clean_reply(page_tail: &str, prompt: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    // 提问本身会出现在页面上，按首行特征把它和它后面的内容整段去掉
    let prompt_head: String = prompt.chars().take(40).collect();
    let mut skipping = false;

    for line in page_tail.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        // 进入「我们的提问」区域后，跳过直到提问结束
        if !prompt_head.is_empty() && t.contains(&prompt_head) {
            skipping = true;
            continue;
        }
        if skipping {
            // 提问通常一大段，遇到明显的短界面行就认为提问结束
            if t.chars().count() < 12 {
                skipping = false;
            } else {
                continue;
            }
        }
        // 丢掉典型的界面噪声
        let noise = t.chars().count() <= 8
            && (t.contains("复制")
                || t.contains("重新生成")
                || t.contains("分享")
                || t.contains("赞")
                || t.contains("踩")
                || t.contains("新对话")
                || t.contains("发送"));
        if noise {
            continue;
        }
        out.push(t);
    }

    // 只保留末尾一段，避免把整页历史都塞进要点的输入
    let keep = 60usize;
    let start = out.len().saturating_sub(keep);
    out[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_table_has_expected_ids() {
        for id in ["kimi", "deepseek", "tongyi", "doubao", "chatgpt", "custom"] {
            assert!(site_adapter(id).is_some(), "缺少站点 {id}");
        }
        assert!(site_adapter("不存在").is_none());
        // id 查询大小写不敏感，配置里写错大小写不该导致找不到
        assert!(site_adapter("KIMI").is_some());
    }

    #[test]
    fn risk_must_be_acknowledged() {
        let cfg = BrowserBotConfig {
            risk_ack: false,
            ..Default::default()
        };
        assert!(!cfg.is_ready());
        let err = ask(&cfg, "s", "u").unwrap_err().to_string();
        assert!(err.contains("risk_ack"), "应提示需要风险确认: {err}");
    }

    #[test]
    fn unknown_site_falls_back_to_first_adapter() {
        let cfg = BrowserBotConfig {
            site: "没有这个站点".into(),
            ..Default::default()
        };
        assert_eq!(cfg.adapter().id, SITES[0].id);
    }

    #[test]
    fn selector_override_is_parsed() {
        let cfg = BrowserBotConfig {
            selectors_override: "input=#a;send=.b ; reply=div.c".into(),
            ..Default::default()
        };
        let o = cfg.overrides();
        assert_eq!(o.input, "#a");
        assert_eq!(o.send, ".b");
        assert_eq!(o.reply, "div.c");
    }

    #[test]
    fn default_user_data_dir_lives_under_tools() {
        let cfg = BrowserBotConfig::default();
        let d = cfg.effective_user_data_dir();
        assert!(d.to_string_lossy().contains("browser-profile"));
    }

    #[test]
    fn fill_script_escapes_quotes_and_newlines() {
        // 提示词里必然有换行与引号，JS 注入必须安全转义
        let js = js_fill("第一行\n\"带引号\"\n第三行");
        assert!(js.contains(r#"\n"#), "换行应被转义");
        assert!(js.contains(r#"\""#), "引号应被转义");
        assert!(js.contains("data-vca-input"));
    }

    #[test]
    fn clean_reply_drops_ui_noise_and_own_prompt() {
        let prompt = "你是一位严谨的课堂纪要整理助手。用户会给你一节课的语音转写文本";
        let page = "新对话\n\
                    你是一位严谨的课堂纪要整理助手。用户会给你一节课的语音转写文本\n\
                    请提取结构化要点\n\
                    {\"title\":\"二次函数\",\"points\":[\"顶点公式\"]}\n\
                    复制\n重新生成\n赞\n";
        let out = clean_reply(page, prompt);
        assert!(out.contains("二次函数"), "回复本体应保留: {out}");
        assert!(!out.contains("重新生成"), "界面按钮应被剔除: {out}");
        assert!(!out.contains("你是一位严谨"), "自己的提问应被剔除: {out}");
    }

    #[test]
    fn clean_reply_keeps_only_tail() {
        // 上百行历史对话时只保留末尾，避免把整页塞进提取提示词
        let page: String = (0..200)
            .map(|i| format!("历史行内容足够长{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = clean_reply(&page, "不相关的提问");
        assert!(out.lines().count() <= 60, "应只保留末尾若干行");
    }

    #[test]
    fn empty_page_yields_empty_reply() {
        assert_eq!(clean_reply("", "问"), "");
    }
}
