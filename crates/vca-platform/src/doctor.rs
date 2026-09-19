//! 环境自检。
//!
//! 检查项分三类：
//!
//! | 类别 | 处理 |
//! |---|---|
//! | **已内置替代**（ffmpeg / LibreOffice） | 不再报错录制用 PyAV、PDF 用 fpdf2，都打包在 `python/vendor` 里 |
//! | 影响功能的外部依赖（Python） | 报告是否可用；缺失时说明用内置运行时 |
//! | 运行环境（会话类型） | 服务模式抓不到屏幕，给出明确告警 |
//!
//! 更深入的录制能力探测（屏幕 / 麦克风）由 Python 侧的 `--selftest` 完成，
//! 因为那需要真正打开设备。

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

/// 运行全部环境自检。
///
/// 注意：**不再把 ffmpeg / LibreOffice 列为必需或缺失**
/// 它们的功能已经内置（PyAV / fpdf2），报「缺失」只会误导用户。
pub fn run_all() -> Vec<CheckItem> {
    vec![
        // Python：录制、转写、文档生成都走它，但**发行版自带嵌入式运行时**，
        // 所以系统里有没有都不影响功能因此始终标记为「可选（仅信息）」。
        {
            let mut it = check_executable("python", "--version");
            it.optional = true;
            if it.ok {
                it.detail = format!("{}（可选：程序自带 python\\runtime）", it.detail);
            } else {
                it.detail = "系统未安装（不影响：程序自带 python\\runtime）".to_string();
            }
            it
        },
        // 以下两项仅供了解，缺失不影响任何功能
        {
            let mut it = check_executable("ffmpeg", "-version");
            it.optional = true;
            it.detail = if it.ok {
                format!("{}（可选：内置 PyAV 已够用）", it.detail)
            } else {
                "未安装（不需要：录制由内置 PyAV 完成）".to_string()
            };
            it
        },
        {
            let mut it = check_executable("soffice", "--version");
            it.optional = true;
            it.detail = if it.ok {
                format!("{}（可选：内置 fpdf2 已够用）", it.detail)
            } else {
                "未安装（不需要：PDF 由内置 fpdf2 生成）".to_string()
            };
            it
        },
    ]
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
    fn all_checks_are_optional_because_they_are_bundled() {
        // ffmpeg / soffice / python 的功能都已经内置（PyAV / fpdf2 / 自带运行时），
        // 所以自检里它们**一律是可选项**，不该因为缺失而判定环境不合格。
        for it in &run_all() {
            assert!(it.optional, "{} 应为可选（功能已内置）", it.name);
        }
    }
}
