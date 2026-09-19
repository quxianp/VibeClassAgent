//! 插件进程运行时：拉起插件子进程并按 **JSON-RPC 2.0 over stdio** 通信。
//!
//! 协议（与 `vca-ipc` 契约一致）：
//! - 每行一个 JSON 对象（NDJSON）；
//! - 方法：`describe` / `init` / `run` / `cleanup` / `health_check`；
//! - 请求  响应按 `id` 配对；本实现为**同步串行**调用（一次一个请求），
//!   因为课后处理本身就是串行的，且能避免读缓冲区的并发复杂度。
//!
//! 隔离性：插件崩溃只影响它自己的进程，宿主通过 `try_wait` 感知并报错，不会连带崩溃。

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use vca_ipc::{methods, DescribeResult, HealthStatus, Request};

use crate::manifest::Manifest;

/// 运行插件时的错误。
#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    /// 进程启动失败。
    #[error("启动插件进程失败: {0}")]
    Spawn(String),
    /// 进程未运行时被调用。
    #[error("插件进程未运行")]
    NotRunning,
    /// 读写失败。
    #[error("与插件通信失败: {0}")]
    Io(String),
    /// 超时。
    #[error("插件调用超时（{0} ms）")]
    Timeout(u64),
    /// 插件返回错误。
    #[error("插件返回错误 {code}: {message}")]
    Rpc {
        /// 错误码。
        code: i32,
        /// 错误信息。
        message: String,
    },
    /// 响应无法解析。
    #[error("插件响应无法解析: {0}")]
    BadResponse(String),
    /// 进程提前退出。
    #[error("插件进程提前退出，代码 {0:?}")]
    Exited(Option<i32>),
}

/// 一个已启动的插件进程。
pub struct PluginProcess {
    manifest: Manifest,
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Option<BufReader<ChildStdout>>,
    next_id: u64,
    timeout: Duration,
}

impl PluginProcess {
    /// 依据清单启动插件进程。
    pub fn start(manifest: &Manifest, plugin_dir: &std::path::Path) -> Result<Self, RunnerError> {
        let entry = plugin_dir.join(&manifest.runtime.entry);
        if !entry.exists() {
            return Err(RunnerError::Spawn(format!(
                "找不到插件入口 {}",
                entry.display()
            )));
        }

        let mut cmd = Command::new(&entry);
        cmd.current_dir(plugin_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        // 静默：子进程不弹任何窗口
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd.spawn().map_err(|e| RunnerError::Spawn(e.to_string()))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().map(BufReader::new);

        Ok(Self {
            manifest: manifest.clone(),
            child,
            stdin,
            stdout,
            next_id: 1,
            timeout: Duration::from_secs(manifest.runtime.timeout_sec.max(1)),
        })
    }

    /// 插件 id。
    pub fn id(&self) -> &str {
        &self.manifest.plugin.id
    }

    /// 进程是否仍在运行。
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// 取进程 pid（日志用）。
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// 发送一次请求并读取响应（同步串行）。
    pub fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, RunnerError> {
        if !self.is_alive() {
            return Err(RunnerError::NotRunning);
        }
        let id = self.next_id;
        self.next_id += 1;

        let req = Request::new(id, method, params);
        let line = serde_json::to_string(&req).map_err(|e| RunnerError::Io(e.to_string()))?;

        {
            let stdin = self.stdin.as_mut().ok_or(RunnerError::NotRunning)?;
            stdin
                .write_all(line.as_bytes())
                .and_then(|_| stdin.write_all(b"\n"))
                .and_then(|_| stdin.flush())
                .map_err(|e| RunnerError::Io(e.to_string()))?;
        }

        let stdout = self.stdout.as_mut().ok_or(RunnerError::NotRunning)?;
        let started = Instant::now();
        let mut buf = String::new();
        loop {
            if started.elapsed() > self.timeout {
                return Err(RunnerError::Timeout(self.timeout.as_millis() as u64));
            }
            buf.clear();
            let n = stdout
                .read_line(&mut buf)
                .map_err(|e| RunnerError::Io(e.to_string()))?;
            if n == 0 {
                let code = self.child.try_wait().ok().flatten().and_then(|s| s.code());
                return Err(RunnerError::Exited(code));
            }
            let t = buf.trim();
            if t.is_empty() {
                continue;
            }
            let v: serde_json::Value = match serde_json::from_str(t) {
                Ok(v) => v,
                // 插件可能打印了非 JSON 的日志行，跳过
                Err(_) => continue,
            };
            // 只接受与本次 id 匹配的响应
            if v.get("id").and_then(|i| i.as_u64()) != Some(id) {
                continue;
            }
            if let Some(err) = v.get("error") {
                return Err(RunnerError::Rpc {
                    code: err.get("code").and_then(|c| c.as_i64()).unwrap_or(-1) as i32,
                    message: err
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("未知错误")
                        .to_string(),
                });
            }
            return Ok(v.get("result").cloned().unwrap_or(serde_json::Value::Null));
        }
    }

    /// 请求插件自述能力。
    pub fn describe(&mut self) -> Result<DescribeResult, RunnerError> {
        let v = self.call(methods::DESCRIBE, serde_json::Value::Null)?;
        serde_json::from_value(v).map_err(|e| RunnerError::BadResponse(e.to_string()))
    }

    /// 初始化并注入配置。
    pub fn init(&mut self, config: serde_json::Value) -> Result<(), RunnerError> {
        let _ = self.call(methods::INIT, config)?;
        Ok(())
    }

    /// 执行主逻辑。
    pub fn run(&mut self, params: serde_json::Value) -> Result<serde_json::Value, RunnerError> {
        self.call(methods::RUN, params)
    }

    /// 健康检查。
    pub fn health_check(&mut self) -> Result<HealthStatus, RunnerError> {
        let v = self.call(methods::HEALTH_CHECK, serde_json::Value::Null)?;
        serde_json::from_value(v).map_err(|e| RunnerError::BadResponse(e.to_string()))
    }

    /// 释放资源并结束进程。
    pub fn shutdown(&mut self) {
        let _ = self.call(methods::CLEANUP, serde_json::Value::Null);
        self.stdin.take();
        // 给插件 2 秒优雅退出
        for _ in 0..20 {
            if !matches!(self.child.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for PluginProcess {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl std::fmt::Debug for PluginProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginProcess")
            .field("id", &self.manifest.plugin.id)
            .field("pid", &self.child.id())
            .finish()
    }
}

/// 把 JSON-RPC 结果裁剪成日志友好的字符串。
pub fn brief(v: &serde_json::Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 200 {
        s.chars().take(200).collect::<String>()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brief_truncates_long_json() {
        let long = serde_json::json!({ "x": "a".repeat(500) });
        assert!(brief(&long).chars().count() <= 201);
    }

    #[test]
    fn brief_keeps_short_json() {
        let v = serde_json::json!({"ok": true});
        assert_eq!(brief(&v), "{\"ok\":true}");
    }

    #[test]
    fn request_new_sets_version() {
        let r = Request::new(7, methods::RUN, serde_json::json!({}));
        assert_eq!(r.jsonrpc, "2.0");
        assert_eq!(r.id, 7);
    }

    #[test]
    fn response_deserialises() {
        let s = r#"{"jsonrpc":"2.0","id":1,"result":{"healthy":true}}"#;
        let r: vca_ipc::Response = serde_json::from_str(s).unwrap();
        assert_eq!(r.id, 1);
        assert_eq!(r.result.get("healthy").unwrap().as_bool(), Some(true));
    }
}
