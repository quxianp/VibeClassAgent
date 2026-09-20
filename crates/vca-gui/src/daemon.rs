//! 在界面里启动 / 停止守护进程。
//!
//! ## 为什么必须有这个
//!
//! 配好模型、推送、课表之后，用户最后一步是「让它开始工作」。
//! 如果这一步只能退出去开一个命令行敲 `vca run`，那界面等于只做了一半 ——
//! 之前那版 CLI 就是这个毛病：配置在界面里，运行得去命令行。
//!
//! ## 线程模型
//!
//! `Daemon::run()` 是阻塞的（内部是个带睡眠的主循环），所以这里把它放到
//! 独立线程里跑；停止走 `vca_platform::shutdown` 的全局标志 —— 那条路径
//! 本来就是给 Ctrl+C 用的，会先停掉正在进行的录制再退出，正好复用。

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use anyhow::Result;
use serde_json::{json, Value};

use vca_core::paths::Layout;
use vca_engine::daemon::{Daemon, DaemonConfig};

/// 守护进程的运行时状态（进程级单例）。
struct Runtime {
    running: bool,
    started_at: Option<Instant>,
    /// 最后一次异常退出的原因，给界面显示。
    last_error: Option<String>,
    /// 这次是以演练模式跑的吗（界面上要标出来，免得以为真在录）。
    dry_run: bool,
}

fn state() -> &'static Mutex<Runtime> {
    static S: OnceLock<Mutex<Runtime>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(Runtime {
            running: false,
            started_at: None,
            last_error: None,
            dry_run: false,
        })
    })
}

/// 启动守护进程。
///
/// `dry_run` 为真时只演练：不真正录制、不真正推送 ——
/// 让用户先看一遍"它到底会干什么"再决定要不要真跑。
pub fn start(layout: Layout, profile: &str, dry_run: bool) -> Result<Value> {
    {
        let s = state().lock().map_err(|_| anyhow::anyhow!("状态锁失效"))?;
        if s.running {
            return Ok(json!({ "ok": false, "error": "守护进程已经在运行了" }));
        }
    }

    // 清掉可能残留的停止标志，否则新起的线程会立刻自己退出
    vca_platform::shutdown::reset();

    let prof = profile.to_string();
    std::thread::Builder::new()
        .name("vca-daemon".to_string())
        .spawn(move || {
            let cfg = DaemonConfig {
                profile: prof,
                dry_run,
                ..Default::default()
            };
            let result = Daemon::load(layout, cfg).and_then(|mut d| d.run());
            let err = result.err().map(|e| e.to_string());
            if let Ok(mut s) = state().lock() {
                s.running = false;
                s.started_at = None;
                s.dry_run = false;
                s.last_error = err.clone();
            }
            match err {
                Some(e) => tracing::error!("守护进程退出：{e}"),
                None => tracing::info!("守护进程已退出"),
            }
        })
        .map_err(|e| anyhow::anyhow!("无法创建守护线程：{e}"))?;

    let mut s = state().lock().map_err(|_| anyhow::anyhow!("状态锁失效"))?;
    s.running = true;
    s.started_at = Some(Instant::now());
    s.dry_run = dry_run;
    s.last_error = None;
    tracing::info!(
        "守护进程已启动{}",
        if dry_run { "（演练模式）" } else { "" }
    );
    Ok(json!({ "ok": true, "running": true, "dry_run": dry_run }))
}

/// 请求停止守护进程。
///
/// 返回的是「已请求」，不是「已停止」—— 它要先停掉正在进行的录制、
/// 把录像文件收尾，这需要几秒。界面看到 running 变 false 才算真的停了。
pub fn stop() -> Result<Value> {
    let running = state().lock().map(|s| s.running).unwrap_or(false);
    if !running {
        return Ok(json!({ "ok": false, "error": "守护进程没有在运行" }));
    }
    vca_platform::shutdown::request_shutdown();
    tracing::info!("已请求停止守护进程（正在收尾，可能要几秒）");
    Ok(json!({
        "ok": true,
        "stopping": true,
        "note": "正在收尾：若此时在录制，会先把录像写完再退出。",
    }))
}

/// 查询状态。
pub fn status() -> Value {
    let s = match state().lock() {
        Ok(s) => s,
        Err(_) => return json!({ "running": false, "error": "状态不可读" }),
    };
    let secs = s.started_at.map(|t| t.elapsed().as_secs()).unwrap_or(0);
    json!({
        "running": s.running,
        "dry_run": s.dry_run,
        "uptime_secs": secs,
        "uptime": humanize(secs),
        "last_error": s.last_error,
    })
}

fn humanize(secs: u64) -> String {
    if secs < 60 {
        format!("{secs} 秒")
    } else if secs < 3600 {
        format!("{} 分 {} 秒", secs / 60, secs % 60)
    } else {
        format!("{} 小时 {} 分", secs / 3600, (secs % 3600) / 60)
    }
}
