//! 前端资源的解析与供给 —— **前后端分离的边界就在这个文件**。
//!
//! # 设计目标
//!
//! 前端要能**独立于 Rust 改动**：另一个 agent 改界面时不应该需要碰 `crates/`，
//! 更不应该每次改一行 CSS 都等一次 `cargo build`。同时发布形态不能退化 ——
//! 用户拿到的仍然可以是「一个 exe + 一个 web 目录」。
//!
//! # 两层来源，外部优先
//!
//! 1. **外部目录**（`web_dir`，来自 `VCA_WEB_DIR` 或程序目录下的 `web/`）：
//!    直接读盘。开发与界面重构都走这条。
//! 2. **内置资源**（`include_str!` 编进 exe）：外部没有时兜底，
//!    保证 exe 单独拷走也能跑起来。
//!
//! 顺序是刻意的：**外部优先**让"改完刷新即见"成立，而内置兜底让"单文件
//! 部署"仍然可行。两者不是二选一。
//!
//! # 为什么不再写死三个文件名
//!
//! 原来这里只有 `INDEX_HTML` / `STYLE_CSS` / `APP_JS` 三个常量，
//! `match` 里也是三条硬编码分支。那样一来前端只要多拆一个 js/css 文件
//! 就得回来改 Rust —— 分离就白做了。现在：
//!
//! - 内置的那几个仍然编进 exe（保证兜底完整）；
//! - 外部目录下的**任意文件**都能被服务，只要路径合法；
//! - 文件名 → Content-Type 走一张小表，加新类型只改 [`content_type`]。
//!
//! # 安全
//!
//! 这个服务只绑回环，但仍然**必须**挡住路径穿越：外部目录是可控的，
//! 一个 `/../config/profiles/default/settings.yaml` 就能把带
//! `preview_token` 的配置读出来。所以 [`safe_join`] 会拒绝任何
//! 含 `..`、绝对路径或盘符的请求。

use std::path::{Component, Path, PathBuf};

/// 编译进去的主页面（外部没有时兜底）。
pub const INDEX_HTML: &str = include_str!("web/index.html");
/// 编译进去的样式。
pub const STYLE_CSS: &str = include_str!("web/style.css");
/// 编译进去的脚本。
pub const APP_JS: &str = include_str!("web/app.js");

/// 内置资源表：外部目录里同名文件会覆盖它。
///
/// 只有这三份是"保证存在"的 —— 它们是界面的最小可用集。
/// 前端后续拆出来的文件放在外部目录里即可，不需要回来登记。
pub const BUILTIN: &[(&str, &str)] = &[
    ("index.html", INDEX_HTML),
    ("style.css", STYLE_CSS),
    ("app.js", APP_JS),
];

/// 按路径取一份资源：**外部文件优先**，找不到才用编译进去的那份。
///
/// 返回 `None` 表示这个路径既不在外部目录里、也不是内置资源 ——
/// 调用方应当回 404，而不是回一个空的 200（那会让浏览器把
/// "文件不存在"和"文件是空的"混为一谈，排查时非常费劲）。
pub fn resolve(external_dir: Option<&Path>, name: &str, builtin: &str) -> String {
    if let Some(dir) = external_dir {
        if let Some(p) = safe_join(dir, name) {
            if let Ok(s) = std::fs::read_to_string(&p) {
                return s;
            }
        }
    }
    builtin.to_string()
}

/// 从外部目录读一份**任意**资源（前端拆分出来的新文件走这条）。
///
/// `rel` 是相对外部目录的路径，允许 `sub/dir/file.js` 这样的子目录 ——
/// 前端按模块拆目录是很自然的事，不该被限制在平铺结构里。
pub fn resolve_external(external_dir: &Path, rel: &str) -> Option<String> {
    let p = safe_join(external_dir, rel)?;
    std::fs::read_to_string(&p).ok()
}

/// 取内置资源（外部目录没有时兜底）。
pub fn builtin_for(rel: &str) -> Option<&'static str> {
    BUILTIN
        .iter()
        .find(|(name, _)| *name == rel)
        .map(|(_, body)| *body)
}

/// 把相对路径安全地拼到根目录下。
///
/// 拒绝的情况（一律返回 `None`）：
/// - 空路径；
/// - 含 `..` 的任何一段（`ParentDir`）；
/// - 绝对路径、盘符前缀（`C:\`）、UNC 前缀；
/// - 根目录段（`/foo`）。
///
/// 允许子目录（`Normal` 段可以有多级）：前端按模块组织目录是正常的，
/// 挡的只是"往上跳"和"跳到别的盘"。
fn safe_join(root: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty() {
        return None;
    }
    // Windows 上 `a\b` 和 `a/b` 都要认，所以先把反斜杠统一成正斜杠再看
    let normalized = rel.replace('\\', "/");
    let candidate = Path::new(&normalized);

    // 逐个组件检查：只允许 Normal（普通名字）和 CurDir（`.`，无害）
    for comp in candidate.components() {
        match comp {
            Component::Normal(_) | Component::CurDir => {}
            // ParentDir(`..`)、RootDir(`/x`)、Prefix(`C:`) 全部拒绝
            _ => return None,
        }
    }

    let joined = root.join(candidate);
    // 双保险：拼完之后必须仍在 root 之下。逐组件检查已经挡住了
    // `..`，但这一步能兜住平台差异（比如某些奇怪的符号链接行为）。
    if !joined.starts_with(root) {
        return None;
    }
    Some(joined)
}

/// 文件名 → Content-Type。
///
/// 表很小是故意的：**只有浏览器会真正按类型处理的才需要列**。
/// 认不出来的类型一律走 `application/octet-stream`，
/// 比猜一个错的 `text/html` 安全（后者会让浏览器执行不该执行的东西）。
pub fn content_type(rel: &str) -> &'static str {
    let lower = rel.to_ascii_lowercase();
    match lower.rsplit('.').next() {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") | Some("mjs") => "application/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("map") => "application/json; charset=utf-8",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// 这份资源是不是"界面入口"（决定要不要走 no-store 之类的特殊处理）。
pub fn is_entry(rel: &str) -> bool {
    matches!(rel, "" | "/" | "index.html" | "/index.html")
}

/// 把请求路径规范成相对资源名。
///
/// `/` 和 `/index.html` 都归一成 `index.html`；其余去掉开头的 `/`。
/// 查询串由调用方先剥掉（这里只管路径部分）。
pub fn normalize_path(path: &str) -> String {
    let p = path.trim_start_matches('/');
    if p.is_empty() {
        "index.html".to_string()
    } else {
        p.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_has_the_three_essentials() {
        // 这三个是"exe 单独拷走也能开界面"的最小集，少一个兜底就不完整
        for name in ["index.html", "style.css", "app.js"] {
            assert!(builtin_for(name).is_some(), "内置资源缺 {name}");
            assert!(!builtin_for(name).unwrap().is_empty(), "{name} 是空的");
        }
    }

    #[test]
    fn builtin_table_and_consts_agree() {
        assert_eq!(builtin_for("index.html"), Some(INDEX_HTML));
        assert_eq!(builtin_for("style.css"), Some(STYLE_CSS));
        assert_eq!(builtin_for("app.js"), Some(APP_JS));
    }

    #[test]
    fn empty_path_is_rejected() {
        assert!(safe_join(Path::new("C:\\web"), "").is_none());
    }

    #[test]
    fn parent_dir_escape_is_rejected() {
        let root = Path::new("C:\\app\\web");
        for bad in [
            "../config/settings.yaml",
            "a/../../b.js",
            "..\\..\\secrets.env",
            "./../x",
        ] {
            assert!(
                safe_join(root, bad).is_none(),
                "路径穿越没被挡住：{bad} —— 能把带 preview_token 的配置读出去"
            );
        }
    }

    #[test]
    fn absolute_and_drive_paths_are_rejected() {
        let root = Path::new("C:\\app\\web");
        for bad in ["/etc/passwd", "C:\\Windows\\win.ini", "\\\\server\\share"] {
            assert!(safe_join(root, bad).is_none(), "绝对路径没被挡住：{bad}");
        }
    }

    #[test]
    fn normal_paths_and_subdirs_are_allowed() {
        let root = Path::new("C:\\app\\web");
        for good in ["app.js", "sub/mod.js", "a/b/c.css", "./style.css"] {
            assert!(
                safe_join(root, good).is_some(),
                "正常的（含子目录）路径被误挡了：{good} —— 前端没法按模块拆目录"
            );
        }
    }

    #[test]
    fn joined_path_stays_under_root() {
        let root = Path::new("C:\\app\\web");
        let p = safe_join(root, "sub/mod.js").unwrap();
        assert!(p.starts_with(root));
    }

    #[test]
    fn resolve_prefers_external_over_builtin() {
        let dir = std::env::temp_dir().join("vca-web-test-override");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("style.css"), "/* 外部版本 */").unwrap();

        let got = resolve(Some(&dir), "style.css", STYLE_CSS);
        assert_eq!(got, "/* 外部版本 */", "外部文件没有优先");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_falls_back_to_builtin() {
        // 目录不存在时应当安静地落回内置版本（发布形态就是这样）
        let dir = std::env::temp_dir().join("vca-web-test-nonexistent-xyz");
        let got = resolve(Some(&dir), "style.css", STYLE_CSS);
        assert_eq!(got, STYLE_CSS);
    }

    #[test]
    fn resolve_without_external_dir_uses_builtin() {
        assert_eq!(resolve(None, "app.js", APP_JS), APP_JS);
    }

    #[test]
    fn content_types_cover_the_web_basics() {
        assert!(content_type("index.html").starts_with("text/html"));
        assert!(content_type("style.css").starts_with("text/css"));
        assert!(content_type("app.js").starts_with("application/javascript"));
        assert!(content_type("mod.mjs").starts_with("application/javascript"));
        assert!(content_type("data.json").starts_with("application/json"));
        assert_eq!(content_type("icon.svg"), "image/svg+xml");
        assert_eq!(content_type("font.woff2"), "font/woff2");
    }

    #[test]
    fn unknown_types_are_not_guessed_as_html() {
        // 认不出来时必须给一个"不会被执行"的类型 ——
        // 猜成 text/html 会让浏览器把任意文件当页面跑
        assert_eq!(content_type("weird.xyz"), "application/octet-stream");
        assert_eq!(content_type("noext"), "application/octet-stream");
    }

    #[test]
    fn content_type_is_case_insensitive() {
        assert!(content_type("APP.JS").starts_with("application/javascript"));
        assert!(content_type("Style.CSS").starts_with("text/css"));
    }

    #[test]
    fn normalize_path_handles_root_and_index() {
        assert_eq!(normalize_path("/"), "index.html");
        assert_eq!(normalize_path("/index.html"), "index.html");
        assert_eq!(normalize_path("index.html"), "index.html");
        assert_eq!(normalize_path("/app.js"), "app.js");
        assert_eq!(normalize_path("/sub/mod.js"), "sub/mod.js");
    }

    #[test]
    fn is_entry_recognises_the_ui_root() {
        assert!(is_entry("/"));
        assert!(is_entry(""));
        assert!(is_entry("index.html"));
        assert!(!is_entry("app.js"));
    }

    #[test]
    fn resolve_external_reads_subdirs() {
        let dir = std::env::temp_dir().join("vca-web-test-subdir");
        let sub = dir.join("components");
        let _ = std::fs::create_dir_all(&sub);
        std::fs::write(sub.join("table.js"), "export const x = 1;").unwrap();

        let got = resolve_external(&dir, "components/table.js");
        assert_eq!(got.as_deref(), Some("export const x = 1;"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_external_refuses_escape() {
        let dir = std::env::temp_dir().join("vca-web-test-escape");
        let _ = std::fs::create_dir_all(&dir);
        assert!(resolve_external(&dir, "../../etc/passwd").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
