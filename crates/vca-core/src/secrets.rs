//! 本地凭据文件 `secrets.env` 的读取与写入。
//!
//! # 为什么不让用户去 setx
//!
//! 环境变量确实更干净，但对老师来说「打开系统设置、新建环境变量、重启程序」
//! 这一串动作太重了 —— 结果就是 Key 一直没配上，功能一直是降级的。
//!
//! 所以在配置目录里放一个 `secrets.env`：
//!
//! - 程序启动时读入，**只补当前还没有的环境变量**（真环境变量优先级更高，
//!   部署方要统一管理时不受影响）；
//! - 文件本身已被 `.gitignore` 覆盖（`secrets.env`、`**/secrets.env`），不会进仓库；
//! - 但它就是**普通文本文件**，这里不假装它加密了。写入时会明确提醒用户
//!   「这个文件里有密钥，别拷给别人」。
//!
//! # 格式
//!
//! 一行一条 `KEY=VALUE`，`#` 开头是注释，值两边的引号会被去掉。
//! 空行忽略。解析失败的行跳过而不是报错 —— 一个文件里的笔误不该让程序起不来。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// 凭据文件名。
pub const SECRETS_FILE: &str = "secrets.env";

/// 进程内的即时覆盖。
///
/// # 为什么需要它（而不是 `std::env::set_var`）
///
/// 用户在界面上粘完 Key，紧接着就要点「拉取模型列表」「发送测试消息」——
/// 那一刻必须已经能读到新 Key。最直觉的做法是 `std::env::set_var`，
/// 但**多线程环境下改环境变量是未定义行为**：守护进程线程可能正好在
/// `getenv`，轻则读到半截字符串、重则更糟。
///
/// 所以这里用一张进程内的表 —— 写入只碰自己的锁。读取优先级是
/// **覆盖表 → 环境变量**。文件仍然照写，下次启动照常从文件加载。
fn overrides() -> &'static Mutex<HashMap<String, String>> {
    static M: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 记一个即时生效的值（界面刚保存密钥时调用）。
pub fn set_override(key: &str, value: &str) {
    if let Ok(mut m) = overrides().lock() {
        m.insert(key.to_string(), value.to_string());
    }
}

/// 读一个密钥：先看即时覆盖，再看环境变量。都没有返回空串。
pub fn get(key: &str) -> String {
    if let Ok(m) = overrides().lock() {
        if let Some(v) = m.get(key) {
            if !v.trim().is_empty() {
                return v.clone();
            }
        }
    }
    std::env::var(key).unwrap_or_default()
}

/// 解析 `KEY=VALUE` 文本。
pub fn parse(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim();
        let v = v.trim().trim_matches('"').trim_matches('\'').trim();
        if k.is_empty() {
            continue;
        }
        out.push((k.to_string(), v.to_string()));
    }
    out
}

/// 读取凭据文件并注入进程环境变量。
///
/// **只补缺失的**：已经存在的环境变量一律不动 —— 否则用户显式设的值
/// 会被这个文件悄悄盖掉，排查起来极其费解。
///
/// 返回注入成功的条数。
pub fn load_into_env(path: &Path) -> usize {
    let Ok(text) = std::fs::read_to_string(path) else {
        return 0;
    };
    let mut n = 0;
    for (k, v) in parse(&text) {
        if v.is_empty() {
            continue;
        }
        if std::env::var_os(&k).is_some() {
            continue;
        }
        std::env::set_var(&k, &v);
        n += 1;
    }
    n
}

/// 写入或更新一条凭据（保留文件里其它条目与注释）。
pub fn upsert(path: &Path, key: &str, value: &str) -> std::io::Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let existing = std::fs::read_to_string(path).unwrap_or_default();

    let header = "# VibeClassAgent 本地凭据\n\
                  # 这个文件里有密钥，不要提交到 Git，也不要拷给别人。\n\
                  # 优先级低于真正的环境变量：同名的环境变量存在时以环境变量为准。\n";
    let mut lines: Vec<String> = Vec::new();
    let mut replaced = false;
    let mut has_any = false;

    for line in existing.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            lines.push(line.to_string());
            continue;
        }
        has_any = true;
        match t.split_once('=') {
            Some((k, _)) if k.trim() == key => {
                lines.push(format!("{key}={value}"));
                replaced = true;
            }
            _ => lines.push(line.to_string()),
        }
    }

    if !replaced {
        if !has_any {
            lines.retain(|l| !l.trim().is_empty());
        }
        lines.push(format!("{key}={value}"));
    }

    let body = if existing.trim().is_empty() {
        format!("{header}{}\n", lines.join("\n"))
    } else {
        format!("{}\n", lines.join("\n"))
    };
    std::fs::write(path, body)
}

/// 默认凭据文件路径：配置根目录下。
pub fn default_path(config_root: &Path) -> PathBuf {
    config_root.join(SECRETS_FILE)
}

/// 某个键当前是否已经可用（即时覆盖、环境变量、凭据文件都算）。
pub fn is_set(key: &str) -> bool {
    !get(key).trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_wins_over_env_and_is_readable() {
        // 覆盖表要能盖住环境变量：界面刚保存的 Key 必须立刻生效
        std::env::set_var("VCA_TEST_OVERRIDE_KEY", "from-env");
        assert_eq!(get("VCA_TEST_OVERRIDE_KEY"), "from-env");
        set_override("VCA_TEST_OVERRIDE_KEY", "from-ui");
        assert_eq!(get("VCA_TEST_OVERRIDE_KEY"), "from-ui");
        // 空值不该盖掉环境变量（界面留空表示"不改"）
        set_override("VCA_TEST_OVERRIDE_KEY", "   ");
        assert_eq!(get("VCA_TEST_OVERRIDE_KEY"), "from-env");
        std::env::remove_var("VCA_TEST_OVERRIDE_KEY");
        assert!(get("VCA_TEST_OVERRIDE_KEY_ABSENT").is_empty());
    }

    #[test]
    fn parses_key_value_with_comments_and_quotes() {
        let text = "# 注释\n\nVCA_LLM_API_KEY=\"sk-abc\"\nOTHER='x'\n坏行\nEMPTY=\n";
        let kv = parse(text);
        assert!(kv.contains(&("VCA_LLM_API_KEY".into(), "sk-abc".into())));
        assert!(kv.contains(&("OTHER".into(), "x".into())));
        assert!(kv.contains(&("EMPTY".into(), "".into())));
        // 没有等号的行要被跳过，而不是让整个解析失败
        assert_eq!(kv.len(), 3);
    }

    #[test]
    fn upsert_appends_then_replaces() {
        let dir = std::env::temp_dir().join(format!("vca_secrets_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join(SECRETS_FILE);

        upsert(&p, "K1", "v1").unwrap();
        upsert(&p, "K2", "v2").unwrap();
        let t = std::fs::read_to_string(&p).unwrap();
        assert!(t.contains("K1=v1"));
        assert!(t.contains("K2=v2"));
        assert!(t.contains("不要提交到 Git"), "应写入安全提示头");

        // 更新已有键不应该产生重复行
        upsert(&p, "K1", "v1-new").unwrap();
        let t = std::fs::read_to_string(&p).unwrap();
        assert!(t.contains("K1=v1-new"));
        assert!(!t.contains("K1=v1\n"), "旧值应被替换而不是并存");
        assert_eq!(t.matches("K1=").count(), 1, "不该出现重复的键");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_does_not_override_existing_env() {
        // 这条是核心保证：显式设过的环境变量不能被凭据文件悄悄盖掉
        let dir = std::env::temp_dir().join(format!("vca_secrets_env_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join(SECRETS_FILE);
        std::fs::create_dir_all(&dir).unwrap();

        std::env::set_var("VCA_TEST_ALREADY_SET", "from-env");
        std::fs::write(
            &p,
            "VCA_TEST_ALREADY_SET=from-file\nVCA_TEST_FRESH=from-file\n",
        )
        .unwrap();

        let n = load_into_env(&p);
        assert_eq!(
            std::env::var("VCA_TEST_ALREADY_SET").unwrap(),
            "from-env",
            "已有环境变量不该被文件覆盖"
        );
        assert_eq!(std::env::var("VCA_TEST_FRESH").unwrap(), "from-file");
        assert_eq!(n, 1, "只有缺失的那条会被注入");

        std::env::remove_var("VCA_TEST_ALREADY_SET");
        std::env::remove_var("VCA_TEST_FRESH");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let n = load_into_env(Path::new("根本不存在的文件.env"));
        assert_eq!(n, 0);
    }
}
