//! 每个推送渠道各自的「地址 + 目标」。
//!
//! # 为什么单独一个文件，而不是并进 settings.yaml
//!
//! 用户报的现象是「每次打开都要重新配置一遍」。实测数据其实**没丢**，
//! 真正的问题是：`push.endpoint` / `push.target` 是**所有渠道共用**的两个字段，
//! 切到另一个渠道试一下再切回来，看到的是别的渠道的值 —— 看起来就像被改掉了。
//!
//! 修法是「每个渠道各记一份」。但这份数据**不能**塞进 settings.yaml：
//! 那边的写入走 `patch_settings_line`，它是「按 a.b.c 逐级缩进定位 + 在段落末尾插入」，
//! 处理不了「新建一个 map 键」—— 写 `push.profiles.onebot.endpoint` 时，
//! `push.profiles.onebot` 这一层还不存在，插入点会落在 `profiles:` 段末，
//! 结果是层级错乱、**写坏用户的配置**。
//!
//! 所以用一个独立文件，由 serde 整体序列化：结构简单，且不可能写歪。
//! settings.yaml 里的 `push.endpoint` / `push.target` 仍然照写 ——
//! daemon 读的还是那一份，这个文件只是界面的「渠道记忆」。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 一个渠道记住的东西。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PushEntry {
    /// 地址（Webhook / 服务基址 / 自建服务器）。
    #[serde(default)]
    pub endpoint: String,
    /// 目标（群号 / chat_id / 会话 id / topic / @手机号）。
    #[serde(default)]
    pub target: String,
    /// 目标类型：`group` 或 `private`。
    #[serde(default)]
    pub target_type: String,
}

/// 全部渠道的记忆，按渠道 id 索引（`onebot` / `wecom` / `dingtalk` / …）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PushProfiles {
    /// 渠道 id -> 该渠道上次用的地址与目标。
    #[serde(default)]
    pub profiles: HashMap<String, PushEntry>,
}

impl PushProfiles {
    /// 从文件读；不存在或读坏了都返回空表。
    ///
    /// **读坏不该让界面起不来** —— 这份数据只影响"输入框里预填什么"，
    /// 丢了大不了重填一次，比让整个推送页打不开强得多。
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_yaml::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// 写回文件。
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let text = serde_yaml::to_string(self).map_err(|e| std::io::Error::other(e.to_string()))?;
        std::fs::write(path, text)
    }

    /// 记下一个渠道当前的地址与目标。
    pub fn remember(&mut self, provider: &str, entry: PushEntry) {
        if provider.trim().is_empty() {
            return;
        }
        self.profiles.insert(provider.trim().to_string(), entry);
    }

    /// 取某个渠道记住的那份（没有就是空的，界面据此清空字段）。
    pub fn get(&self, provider: &str) -> PushEntry {
        self.profiles
            .get(provider.trim())
            .cloned()
            .unwrap_or_default()
    }
}

/// 渠道记忆文件的默认位置：配置根目录下。
pub fn default_path(config_root: &Path) -> PathBuf {
    config_root.join("push-profiles.yaml")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vca-pushprof-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("push-profiles.yaml")
    }

    #[test]
    fn remember_and_read_back() {
        let mut p = PushProfiles::default();
        p.remember(
            "dingtalk",
            PushEntry {
                endpoint: "https://oapi.dingtalk.com/robot/send?access_token=x".into(),
                target: "13800000000".into(),
                target_type: "group".into(),
            },
        );
        let got = p.get("dingtalk");
        assert!(got.endpoint.contains("dingtalk"));
        assert_eq!(got.target, "13800000000");
    }

    /// 这是修缺陷的核心：两个渠道各记各的，互不覆盖。
    #[test]
    fn channels_do_not_overwrite_each_other() {
        let mut p = PushProfiles::default();
        p.remember(
            "onebot",
            PushEntry {
                endpoint: "http://127.0.0.1:3000".into(),
                target: "123456".into(),
                target_type: "group".into(),
            },
        );
        p.remember(
            "dingtalk",
            PushEntry {
                endpoint: "https://oapi.dingtalk.com/robot/send?access_token=y".into(),
                target: "13900000000".into(),
                target_type: "group".into(),
            },
        );

        assert_eq!(p.get("onebot").endpoint, "http://127.0.0.1:3000");
        assert_eq!(p.get("onebot").target, "123456");
        assert!(p.get("dingtalk").endpoint.contains("dingtalk"));
        assert_eq!(p.get("dingtalk").target, "13900000000");
    }

    #[test]
    fn unknown_channel_reads_as_empty_not_as_someone_elses() {
        // 没配过的渠道必须是空的 —— 绝不能返回另一个渠道的值（那正是原来的毛病）
        let mut p = PushProfiles::default();
        p.remember(
            "onebot",
            PushEntry {
                endpoint: "http://127.0.0.1:3000".into(),
                target: "1".into(),
                target_type: "group".into(),
            },
        );
        let fresh = p.get("telegram");
        assert_eq!(fresh.endpoint, "");
        assert_eq!(fresh.target, "");
    }

    #[test]
    fn round_trips_through_a_file() {
        let f = tmp("roundtrip");
        let mut p = PushProfiles::default();
        p.remember(
            "feishu",
            PushEntry {
                endpoint: "https://open.feishu.cn/open-apis/bot/v2/hook/abc".into(),
                target: String::new(),
                target_type: String::new(),
            },
        );
        p.save(&f).unwrap();

        let back = PushProfiles::load(&f);
        assert_eq!(back.get("feishu").endpoint, p.get("feishu").endpoint);
        let _ = std::fs::remove_dir_all(f.parent().unwrap());
    }

    #[test]
    fn missing_file_reads_as_empty() {
        let p = PushProfiles::load(Path::new("Z:\\definitely-not-here\\push-profiles.yaml"));
        assert!(p.profiles.is_empty());
    }

    #[test]
    fn broken_file_does_not_panic() {
        // 手改坏了也不该让界面打不开
        let f = tmp("broken");
        std::fs::write(&f, "这不是 yaml: [unclosed").unwrap();
        let p = PushProfiles::load(&f);
        assert!(p.profiles.is_empty());
        let _ = std::fs::remove_dir_all(f.parent().unwrap());
    }

    #[test]
    fn blank_provider_is_ignored() {
        let mut p = PushProfiles::default();
        p.remember("", PushEntry::default());
        p.remember("   ", PushEntry::default());
        assert!(p.profiles.is_empty());
    }
}
