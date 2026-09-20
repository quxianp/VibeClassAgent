//! 前端资源：全部 `include_str!` 进二进制。
//!
//! 这样发布目录里没有前端文件要管，改页面也不需要用户重新安装任何东西；
//! 想临时改样式，把同名文件放到 `assets/web/` 下即可覆盖（见 [`resolve`]）。

use std::path::PathBuf;

/// 编译进去的主页面。
pub const INDEX_HTML: &str = include_str!("web/index.html");
/// 编译进去的样式。
pub const STYLE_CSS: &str = include_str!("web/style.css");
/// 编译进去的脚本。
pub const APP_JS: &str = include_str!("web/app.js");

/// 取一份资源：**外部文件优先**，找不到才用编译进去的那份。
///
/// 顺序是刻意的：开发时改 `assets/web/style.css` 刷新就能看到效果，
/// 不用重新编译；而发布版里这些文件不存在，自动落回内置版本。
pub fn resolve(external_dir: Option<&std::path::Path>, name: &str, builtin: &str) -> String {
    if let Some(dir) = external_dir {
        let p: PathBuf = dir.join(name);
        if let Ok(s) = std::fs::read_to_string(&p) {
            return s;
        }
    }
    builtin.to_string()
}
