//! 示例插件：一个**完整可跑**的最小实现。
//!
//! # 它存在的意义
//!
//! `plugins/` 下那九个内置插件此前只有清单和说明，`entry` 指向并不存在的可执行体 ——
//! 整个「插件架构」是一套没有零件的机器。这个 crate 就是第一件零件：
//! 宿主可以真的把它拉起来，调通 `describe` / `init` / `health_check` / `run` / `cleanup`
//! 五个方法，走完 JSON-RPC over stdio 的完整往返。
//!
//! # 它做什么
//!
//! 功能上它只是把 `run` 收到的参数原样回显（`example.echo`）。
//! 但**协议部分是真的**，想写自己的插件时照抄这个文件即可。
//!
//! # 协议
//!
//! 宿主与插件之间只有两条约定：
//!
//! 1. 插件是**独立进程**，从 stdin 逐行读 JSON-RPC 请求，把响应逐行写到 stdout；
//! 2. 必须实现五个方法：`describe` / `init` / `run` / `cleanup` / `health_check`。
//!
//! 因此插件**不限语言**：Python、Go、Node、C++ 都能写，
//! 只要会用标准输入输出就行。这也是当初选「进程 + stdio」而不是动态库的原因。
//!
//! # 日志约定
//!
//! **stdout 只能走协议**，任何调试输出都必须写 stderr ——
//! 往 stdout 打一行多余的字，宿主就会把它当成非法 JSON 而判定插件故障。

use std::io::{BufRead, Write};

use serde_json::{json, Value};

/// 插件标识（与 manifest 的 id 保持一致）。
const PLUGIN_ID: &str = "example.echo";
/// 插件类型。
const PLUGIN_KIND: &str = "pusher";
/// 插件版本。
const PLUGIN_VERSION: &str = "0.1.0";
/// 协议版本：宿主会核对大版本，不匹配就拒绝加载。
const API_VERSION: &str = "1";

fn main() {
    // 注意：这里用 stderr 做启动日志，stdout 留给协议
    eprintln!("[example.echo] 启动，等待 JSON-RPC 请求（stdin）");

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[example.echo] 读取 stdin 失败: {e}");
                break;
            }
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let req: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                // 协议层解析失败：回一个错误响应而不是退出，
                // 让宿主那边能看到原因，而不是「插件莫名其妙没了」
                eprintln!("[example.echo] 非法 JSON: {e}");
                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": { "code": -32700, "message": format!("解析失败: {e}") }
                });
                let _ = writeln!(out, "{resp}");
                let _ = out.flush();
                continue;
            }
        };

        let id = req.get("id").cloned().unwrap_or(Value::Null);
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(Value::Null);

        let result = match method {
            "describe" => json!({
                "id": PLUGIN_ID,
                "kind": PLUGIN_KIND,
                "version": PLUGIN_VERSION,
                "api_version": API_VERSION,
                "config_schema": {},
                "capabilities": ["echo"]
            }),
            "init" => {
                // 真实插件会在这里读配置、准备资源。
                // 凭据永远从环境变量来，不要从 params 里塞明文。
                let profile = params
                    .get("profile")
                    .and_then(|p| p.as_str())
                    .unwrap_or("default");
                eprintln!("[example.echo] init profile={profile}");
                json!({ "ok": true, "profile": profile })
            }
            "health_check" => json!({ "healthy": true, "message": "就绪" }),
            "cleanup" => {
                eprintln!("[example.echo] cleanup");
                json!({ "ok": true })
            }
            "run" => {
                let kind = params.get("kind").and_then(|k| k.as_str()).unwrap_or("");
                if kind != "push" {
                    json!({ "success": false, "error": format!("本插件只处理 push，收到 {kind}") })
                } else {
                    let title = params
                        .get("title")
                        .and_then(|t| t.as_str())
                        .unwrap_or("(无标题)");
                    eprintln!("[example.echo] 收到推送请求：{title}");
                    json!({
                        "success": true,
                        "messageId": format!("{PLUGIN_ID}-{}", uuid_like()),
                        "echo": params
                    })
                }
            }
            other => json!({ "error": format!("未知方法: {other}") }),
        };

        let resp = json!({ "jsonrpc": "2.0", "id": id, "result": result });
        if writeln!(out, "{resp}").is_err() || out.flush().is_err() {
            // stdout 断了说明宿主已经走了，此时退出即可
            eprintln!("[example.echo] stdout 已关闭，退出");
            break;
        }
    }
}

/// 生成一个够用的唯一串（不引依赖：这里只需要「同一次运行内不重复」）。
fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    format!("{pid:x}-{nanos:x}")
}
