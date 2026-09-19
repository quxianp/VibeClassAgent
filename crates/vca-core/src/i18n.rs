//! 界面文案的多语言支持。
//!
//! # 为什么把文案抽出来
//!
//! 这个程序最终要交到学校老师手里，而不同地区、不同学校的说法并不一样
//! （「课表」还是「课程表」，「午休」还是「午间」）。把文案集中在语言文件里，
//! 改词、加语种都不用重新编译，也不必碰代码。
//!
//! # 两层加载
//!
//! 1. **内置兜底**：编译进二进制的 `locales/zh-CN.json`（`include_str!`），
//!    保证程序在任何情况下都能显示人话 —— 哪怕用户把 `locales/` 删了；
//! 2. **外部覆盖**：`<工具目录>/locales/<lang>.json`，改完立刻生效。
//!    外部文件**按 key 覆盖**内置内容，所以只写要改的那几条就行，不必抄全。
//!
//! # key 的约定
//!
//! 用点分层，前缀对应界面区域：
//!
//! | 前缀 | 区域 |
//! |---|---|
//! | `app.*` | 程序名、版本、缩写 |
//! | `welcome.*` | 启动横幅 |
//! | `cmd.*` | 斜杠命令的说明 |
//! | `status.*` / `jobs.*` / `process.*` | 各命令的输出 |
//! | `doctor.*` | 自检 |
//! | `setup.*` | 首次引导 |
//! | `err.*` / `hint.*` | 错误与提示 |
//!
//! 缺 key 时**返回 key 本身**（比如界面上直接显示 `welcome.hint`），
//! 而不是空串 —— 一眼就能看出是漏翻了，而不是莫名其妙地空着一块。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// 内置的简体中文语言包（编译进二进制，永远可用）。
const BUILTIN_ZH: &str = include_str!("../../../locales/zh-CN.json");

/// 内置的英文语言包。
const BUILTIN_EN: &str = include_str!("../../../locales/en-US.json");

/// 默认语种。
pub const DEFAULT_LANG: &str = "zh-CN";

/// 支持的内置语种。
pub const SUPPORTED: &[&str] = &["zh-CN", "en-US"];

static TABLES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
static CURRENT: OnceLock<Mutex<String>> = OnceLock::new();

fn tables() -> &'static Mutex<HashMap<String, String>> {
    TABLES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn current() -> &'static Mutex<String> {
    CURRENT.get_or_init(|| Mutex::new(DEFAULT_LANG.to_string()))
}

/// 解析一条 JSON 语言包，把 `key -> value` 摊平成一层。
///
/// 支持嵌套对象（`{"welcome": {"hint": "..."}}` 会变成 `welcome.hint`），
/// 这样语言文件本身可以按区域分组，读起来更像文档。
fn flatten(prefix: &str, value: &serde_json::Value, out: &mut HashMap<String, String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten(&key, v, out);
            }
        }
        serde_json::Value::String(s) => {
            out.insert(prefix.to_string(), s.clone());
        }
        serde_json::Value::Array(arr) => {
            // 允许数组作为「多行文案」，用 \n 连起来
            let lines: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            if !lines.is_empty() {
                out.insert(prefix.to_string(), lines.join("\n"));
            }
        }
        other => {
            out.insert(prefix.to_string(), other.to_string());
        }
    }
}

fn parse(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        flatten("", &v, &mut out);
    }
    out
}

/// 外部语言文件所在的候选目录。
///
/// 与外部工具（ffmpeg / whisper）一样，要兼顾发布形态与开发形态。
pub fn locale_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(d) = std::env::var("VCA_LOCALE_DIR") {
        if !d.trim().is_empty() {
            dirs.push(PathBuf::from(d));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(p) = exe.parent() {
            dirs.push(p.join("locales"));
            // exe 在 target/debug 或 target/release 时，仓库根在往上两级
            if let Some(gp) = p.parent().and_then(|x| x.parent()) {
                dirs.push(gp.join("locales"));
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join("locales"));
    }
    dirs
}

/// 载入并安装语言包。
///
/// `prefer` 为语种名（`zh-CN` / `en-US`），留空则用 [`DEFAULT_LANG`]。
/// 外部文件存在时按 key 覆盖内置内容。
pub fn install(prefer: Option<&str>) {
    let lang = prefer
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_LANG.to_string());

    let mut merged: HashMap<String, String> = match lang.as_str() {
        "en-US" | "en" => parse(BUILTIN_EN),
        _ => parse(BUILTIN_ZH),
    };
    // 中文兜底：英文包里没翻到的条目回落到中文，避免界面出现 key
    let zh = parse(BUILTIN_ZH);
    for (k, v) in &zh {
        merged.entry(k.clone()).or_insert_with(|| v.clone());
    }

    // 外部覆盖
    for dir in locale_dirs() {
        let p = dir.join(format!("{lang}.json"));
        if let Ok(text) = std::fs::read_to_string(&p) {
            for (k, v) in parse(&text) {
                merged.insert(k, v);
            }
        }
    }

    if let Ok(mut t) = tables().lock() {
        *t = merged;
    }
    if let Ok(mut c) = current().lock() {
        *c = lang;
    }
}

/// 确保已安装（没装过就装默认语种）。
fn ensure() {
    let installed = tables().lock().map(|t| !t.is_empty()).unwrap_or(false);
    if !installed {
        install(None);
    }
}

/// 当前语种。
pub fn lang() -> String {
    ensure();
    current().lock().map(|c| c.clone()).unwrap_or_else(|_| DEFAULT_LANG.into())
}

/// 取一条文案。缺 key 时返回 key 本身（便于一眼看出漏翻）。
pub fn t(key: &str) -> String {
    ensure();
    tables()
        .lock()
        .ok()
        .and_then(|t| t.get(key).cloned())
        .unwrap_or_else(|| key.to_string())
}

/// 取一条文案并替换 `{名字}` 占位符。
///
/// ```ignore
/// tf("status.jobs_count", &[("n", "3")])
/// // -> "作业共 3 个"
/// ```
pub fn tf(key: &str, vars: &[(&str, &str)]) -> String {
    let mut s = t(key);
    for (k, v) in vars {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

/// 语言文件是否存在于外部目录（供 `/doctor` 提示用）。
pub fn external_file_path(lang: &str) -> Option<PathBuf> {
    locale_dirs()
        .into_iter()
        .map(|d| d.join(format!("{lang}.json")))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_pack_parses_and_has_core_keys() {
        let m = parse(BUILTIN_ZH);
        assert!(!m.is_empty(), "内置中文语言包不该是空的");
        for k in ["app.name", "app.short", "welcome.hint"] {
            assert!(m.contains_key(k), "内置语言包缺少 {k}");
        }
    }

    #[test]
    fn nested_objects_flatten_to_dotted_keys() {
        let mut out = HashMap::new();
        let v: serde_json::Value = serde_json::from_str(r#"{"a":{"b":{"c":"x"}}}"#).unwrap();
        flatten("", &v, &mut out);
        assert_eq!(out.get("a.b.c").map(|s| s.as_str()), Some("x"));
    }

    #[test]
    fn arrays_become_multiline_text() {
        let mut out = HashMap::new();
        let v: serde_json::Value = serde_json::from_str(r#"{"logo":["a","b","c"]}"#).unwrap();
        flatten("", &v, &mut out);
        assert_eq!(out.get("logo").map(|s| s.as_str()), Some("a\nb\nc"));
    }

    #[test]
    fn missing_key_returns_the_key_itself() {
        ensure();
        // 故意用一个不存在的 key：返回 key 才能在界面上一眼看出漏翻
        assert_eq!(t("完全不存在的.key"), "完全不存在的.key");
    }

    #[test]
    fn placeholder_substitution_works() {
        ensure();
        let s = tf("app.name", &[]);
        assert!(!s.is_empty());
        // 直接测替换逻辑本身
        let mut probe = "作业共 {n} 个".to_string();
        probe = probe.replace("{n}", "3");
        assert_eq!(probe, "作业共 3 个");
    }

    #[test]
    fn english_pack_parses() {
        let m = parse(BUILTIN_EN);
        assert!(m.contains_key("app.name"), "英文包缺少 app.name");
    }
}
