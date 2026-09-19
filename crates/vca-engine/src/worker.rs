//! Python 算法侧 Worker 的 stdio JSON-RPC 客户端。
//!
//! Worker 启动方式：`python -m vca_worker --stdio`（工作目录为仓库的 `python/`）。
//! 协议与插件一致（NDJSON + JSON-RPC 2.0），因此复用同一套消息结构。

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use vca_ipc::{methods, Request};

/// Worker 调用错误。
#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    /// 启动失败。
    #[error("启动 Python Worker 失败（请确认已安装 Python 且工作目录正确）: {0}")]
    Spawn(String),
    /// 通信失败。
    #[error("与 Worker 通信失败: {0}")]
    Io(String),
    /// 超时。
    #[error("Worker 调用超时（{0} 秒）")]
    Timeout(u64),
    /// Worker 返回错误。
    #[error("Worker 返回错误 {code}: {message}")]
    Rpc {
        /// 错误码。
        code: i32,
        /// 信息。
        message: String,
    },
    /// 进程退出。
    #[error("Worker 提前退出，代码 {0:?}")]
    Exited(Option<i32>),
}

/// 决定用哪个 Python 解释器。
///
/// 优先级：
/// 1. 环境变量 `VCA_PYTHON`（部署方显式指定）；
/// 2. **随包携带的嵌入式运行时** `python/runtime/python.exe`（推荐，完全自包含）；
/// 3. 系统 PATH 里的 `python`（开发机场景）。
pub fn resolve_python(python_dir: &std::path::Path) -> String {
    if let Ok(p) = std::env::var("VCA_PYTHON") {
        if !p.trim().is_empty() {
            return p;
        }
    }
    let bundled = python_dir.join("runtime").join("python.exe");
    if bundled.is_file() {
        return bundled.to_string_lossy().to_string();
    }
    "python".to_string()
}

/// Worker 客户端。
pub struct WorkerClient {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Option<BufReader<ChildStdout>>,
    next_id: u64,
    timeout: Duration,
}

impl WorkerClient {
    /// 启动 Worker。
    ///
    /// `python_dir` 应指向仓库里的 `python/` 目录（含 `vca_worker` 包）。
    pub fn start(python_dir: &std::path::Path, timeout_secs: u64) -> Result<Self, WorkerError> {
        let python = resolve_python(python_dir);
        let mut cmd = Command::new(&python);
        cmd.arg("-m")
            .arg("vca_worker")
            .arg("--stdio")
            .current_dir(python_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd.spawn().map_err(|e| WorkerError::Spawn(e.to_string()))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().map(BufReader::new);
        Ok(Self {
            child,
            stdin,
            stdout,
            next_id: 1,
            timeout: Duration::from_secs(timeout_secs.max(5)),
        })
    }

    /// 是否仍存活。
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// 调用一个方法。
    pub fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, WorkerError> {
        if !self.is_alive() {
            return Err(WorkerError::Exited(
                self.child.try_wait().ok().flatten().and_then(|s| s.code()),
            ));
        }
        let id = self.next_id;
        self.next_id += 1;
        let req = Request::new(id, method, params);
        let line = serde_json::to_string(&req).map_err(|e| WorkerError::Io(e.to_string()))?;
        {
            let stdin = self.stdin.as_mut().ok_or(WorkerError::Exited(None))?;
            stdin
                .write_all(line.as_bytes())
                .and_then(|_| stdin.write_all(b"\n"))
                .and_then(|_| stdin.flush())
                .map_err(|e| WorkerError::Io(e.to_string()))?;
        }

        let stdout = self.stdout.as_mut().ok_or(WorkerError::Exited(None))?;
        let started = Instant::now();
        let mut buf = String::new();
        loop {
            if started.elapsed() > self.timeout {
                return Err(WorkerError::Timeout(self.timeout.as_secs()));
            }
            buf.clear();
            let n = stdout
                .read_line(&mut buf)
                .map_err(|e| WorkerError::Io(e.to_string()))?;
            if n == 0 {
                return Err(WorkerError::Exited(
                    self.child.try_wait().ok().flatten().and_then(|s| s.code()),
                ));
            }
            let t = buf.trim();
            if t.is_empty() {
                continue;
            }
            let v: serde_json::Value = match serde_json::from_str(t) {
                Ok(v) => v,
                Err(_) => continue, // 跳过非 JSON 的日志行
            };
            if v.get("id").and_then(|i| i.as_u64()) != Some(id) {
                continue;
            }
            if let Some(err) = v.get("error") {
                return Err(WorkerError::Rpc {
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

    /// 自检。
    pub fn health(&mut self) -> Result<bool, WorkerError> {
        let v = self.call(methods::HEALTH_CHECK, serde_json::Value::Null)?;
        Ok(v.get("healthy").and_then(|h| h.as_bool()).unwrap_or(false))
    }
}

impl Drop for WorkerClient {
    fn drop(&mut self) {
        self.stdin.take();
        for _ in 0..10 {
            if !matches!(self.child.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_fails_gracefully_when_python_missing() {
        // 把 python 指到一个不存在的可执行体，应得到 Spawn 错误而不是 panic
        std::env::set_var("VCA_PYTHON", "definitely-not-a-real-python-binary-xyz");
        let r = WorkerClient::start(std::path::Path::new("."), 5);
        std::env::remove_var("VCA_PYTHON");
        assert!(matches!(r, Err(WorkerError::Spawn(_))));
    }
}
