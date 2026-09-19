//! 环境自检。
//!
//! 检查的是这个程序**真正依赖**的东西：
//!
//! | 项 | 必需？ | 缺失后果 |
//! |---|---|---|
//! | ffmpeg | **是** | 录不了像（屏幕采集与音视频合并都靠它） |
//! | whisper.cpp | 否 | 本地转写不可用，可改走云端 |
//! | 语音模型 | 否 | 同上 |
//! | 浏览器（Edge） | 否 | PDF 退化为保留 HTML；浏览器模式不可用 |
//! | 运行会话 | **是** | 服务模式（Session 0）抓不到屏幕 |
//!
//! 这里**不再检查 Python 与 LibreOffice** —— 那是上一版架构的遗留。
//! 现在整个程序是纯 Rust 的，把它们列出来只会让用户以为缺了东西。

use std::process::Command;

/// 单项检查结果。
#[derive(Debug, Clone)]
pub struct CheckItem {
    /// 检查项名称。
    pub name: String,
    /// 是否通过。
    pub ok: bool,
    /// 说明（版本号或失败原因）。
    pub detail: String,
    /// 是否是「缺失也无所谓」的项。
    pub optional: bool,
}

impl CheckItem {
    /// 面向用户的显示行。
    pub fn render(&self) -> String {
        let mark = match (self.ok, self.optional) {
            (true, _) => "OK  ",
            (false, true) => "SKIP",
            (false, false) => "MISS",
        };
        format!("[{mark}] {:<10} {}", self.name, self.detail)
    }
}

/// 检查一个可执行文件是否可用，并取回其版本首行。
pub fn check_executable(name: &str, version_arg: &str) -> CheckItem {
    match Command::new(name).arg(version_arg).output() {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            let first = text.lines().next().unwrap_or("").trim().to_string();
            CheckItem {
                name: name.to_string(),
                ok: true,
                detail: if first.is_empty() {
                    "已安装".to_string()
                } else {
                    first
                },
                optional: false,
            }
        }
        Ok(_) => CheckItem {
            name: name.to_string(),
            ok: false,
            detail: "命令存在但返回非零状态".to_string(),
            optional: false,
        },
        Err(_) => CheckItem {
            name: name.to_string(),
            ok: false,
            detail: "未找到（不在 PATH 中）".to_string(),
            optional: false,
        },
    }
}

/// 取某个可执行体版本首行。
fn version_of(exe: &std::path::Path, arg: &str) -> String {
    let mut cmd = crate::proc::silent(exe);
    cmd.arg(arg);
    match cmd.output() {
        Ok(o) => {
            // 有的程序把版本打到 stderr（whisper.cpp 就是这样），两边都看
            let out = String::from_utf8_lossy(&o.stdout);
            let err = String::from_utf8_lossy(&o.stderr);
            let line = out
                .lines()
                .chain(err.lines())
                .map(|l| l.trim())
                .find(|l| !l.is_empty())
                .unwrap_or("");
            line.chars().take(70).collect()
        }
        Err(_) => String::new(),
    }
}

/// 按「找到的路径」构造一项检查结果。
fn check_tool(
    name: &str,
    path: Option<std::path::PathBuf>,
    version_arg: &str,
    optional: bool,
    ok_note: &str,
    miss_note: &str,
) -> CheckItem {
    match path {
        Some(p) => {
            let v = version_of(&p, version_arg);
            CheckItem {
                name: name.to_string(),
                ok: true,
                detail: if v.is_empty() {
                    format!("就绪 {ok_note}")
                } else {
                    format!("{v} {ok_note}")
                },
                optional,
            }
        }
        None => CheckItem {
            name: name.to_string(),
            ok: false,
            detail: miss_note.to_string(),
            optional,
        },
    }
}

/// 运行全部环境自检。
///
/// 检查的是**这个程序真正依赖的东西**：
/// ffmpeg（录屏与合并）、whisper.cpp（本地转写）、语音模型、浏览器（PDF 与浏览器模式）。
///
/// 刻意**不再检查 Python / LibreOffice** —— 那是上一版架构的遗留，
/// 现在整个程序是纯 Rust 的，报它们只会让用户以为缺东西。
pub fn run_all() -> Vec<CheckItem> {
    let mut items = Vec::new();

    // 1) ffmpeg：屏幕采集与音视频合并，缺了录不了像（必需）
    items.push(check_tool(
        "ffmpeg",
        crate::capture::ffmpeg_path(),
        "-version",
        false,
        "（录屏与合并）",
        "未找到。运行 scripts/fetch-deps.py 获取，或用 VCA_FFMPEG 指定路径",
    ));

    // 2) whisper.cpp 可执行体：本地转写。缺了还能走云端，所以是可选项
    items.push(check_tool(
        "whisper",
        crate::stt::find_whisper(),
        "--help",
        true,
        "（本地转写）",
        "未找到。运行 scripts/fetch-deps.py 获取；或改用云端转写",
    ));

    // 3) 语音模型：和可执行体是一对，分开报是为了方便定位是哪一半缺了
    items.push({
        let model = std::env::var("VCA_WHISPER_MODEL")
            .ok()
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_file())
            .or_else(|| crate::stt::find_model("tiny"));
        match model {
            Some(p) => {
                let mb = std::fs::metadata(&p)
                    .map(|m| m.len() / 1048576)
                    .unwrap_or(0);
                CheckItem {
                    name: "语音模型".to_string(),
                    ok: true,
                    detail: format!(
                        "{} 已就绪（{mb} MB）",
                        p.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default()
                    ),
                    optional: true,
                }
            }
            None => CheckItem {
                name: "语音模型".to_string(),
                ok: false,
                detail: "未找到 ggml-*.bin。运行 scripts/fetch-deps.py --models tiny".to_string(),
                optional: true,
            },
        }
    });

    // 4) 浏览器：PDF 生成（HTML → 无头打印）与浏览器自动化模式都靠它
    items.push(check_tool(
        "浏览器",
        crate::browser_bot::find_system_browser(),
        "--version",
        true,
        "（生成 PDF / 浏览器模式）",
        "未找到 Edge 或 Chrome。PDF 会退化为保留 HTML，其它功能不受影响",
    ));

    items
}

/// 会话类型检查（服务模式抓不到屏幕）。
pub fn session_check() -> CheckItem {
    let mode = crate::session::detect_run_mode();
    CheckItem {
        name: "运行会话".to_string(),
        ok: !matches!(mode, crate::session::RunMode::Session0),
        detail: mode.label().to_string(),
        optional: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_optional_shows_skip_not_miss() {
        let it = CheckItem {
            name: "x".into(),
            ok: false,
            detail: "d".into(),
            optional: true,
        };
        assert!(it.render().starts_with("[SKIP]"));
    }

    #[test]
    fn missing_required_shows_miss() {
        let it = CheckItem {
            name: "x".into(),
            ok: false,
            detail: "d".into(),
            optional: false,
        };
        assert!(it.render().starts_with("[MISS]"));
    }

    #[test]
    fn ok_shows_ok_regardless_of_optional() {
        for optional in [true, false] {
            let it = CheckItem {
                name: "x".into(),
                ok: true,
                detail: "d".into(),
                optional,
            };
            assert!(it.render().starts_with("[OK  ]"));
        }
    }

    #[test]
    fn only_ffmpeg_is_required() {
        // ffmpeg 缺了录不了像，必须报 MISS；
        // 其余（whisper / 模型 / 浏览器）都有替代路径，只该报 SKIP，
        // 否则用户会以为环境不合格。
        let items = run_all();
        let required: Vec<&str> = items
            .iter()
            .filter(|i| !i.optional)
            .map(|i| i.name.as_str())
            .collect();
        assert_eq!(
            required,
            vec!["ffmpeg"],
            "只有 ffmpeg 必需，实际 {required:?}"
        );
    }

    #[test]
    fn doctor_checks_the_new_architecture() {
        // 自检列表不该再出现上一版架构的遗留项
        let names: Vec<String> = run_all().into_iter().map(|i| i.name).collect();
        assert!(names.iter().any(|n| n == "ffmpeg"));
        assert!(names.iter().any(|n| n == "whisper"));
        assert!(
            !names.iter().any(|n| n == "python" || n == "soffice"),
            "不该再检查 Python/LibreOffice：现在是纯 Rust 实现，实际 {names:?}"
        );
    }
}
