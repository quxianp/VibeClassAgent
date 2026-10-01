//! 令牌生成：界面会话令牌、预览令牌、OneBot HTTP 令牌。
//!
//! # 为什么单独一个模块
//!
//! 这三个令牌的用途不同（会话态 / 长期有效 / 交给机器人），但**要求完全一样**：
//! 不可预测。之前三处各自写了一遍「时间戳 ^ pid + 位混合」，看着够乱，
//! 实际上没有熵来源 —— 时间戳可猜，pid 可枚举，同一毫秒内的候选空间很小。
//! 长期有效的预览令牌尤其危险：它放在 URL 里发给群里所有人。
//!
//! 现在统一走系统 CSPRNG（`rand` 0.10 的 [`SysRng`]，Windows 上即
//! `BCryptGenRandom`），只有一处实现，也就只有一处需要审查。

use rand::rand_core::TryRng;

/// 令牌长度（hex 字符数）：16 字节 = 128 bit 熵 = 32 个字符。
///
/// 128 bit 是这类「本机防误用 + 长期链接防猜」场景的通行下限：
/// 暴力枚举在物理上不可行，而 32 字符塞进 URL 也不算难看。
const TOKEN_BYTES: usize = 16;

/// 生成一个 32 字符的十六进制令牌（128 bit 熵）。
///
/// # 取不到系统随机数时怎么办
///
/// `SysRng` 是会失败的（理论上系统调用出错）。这里的取舍是
/// **让程序停下来**而不是退回弱随机 —— 一个"看起来正常但可预测"的
/// 令牌比启动失败危险得多：用户会以为它是安全的，照常发布预览链接。
///
/// 所以返回空串表示失败，由调用方决定怎么处理（界面会在启动时报警并拒绝
/// 预览请求）。绝不静默降级到时间戳方案。
///
/// 不 panic 是有意的：本 crate 的定位是纯逻辑层，工具链层面禁止 panic
/// 传播到无人值守的守护进程里。
pub fn generate() -> String {
    generate_bytes(TOKEN_BYTES)
}

/// 按给定字节数生成令牌（想要更长时用）。
///
/// 失败返回空串，语义同 [`generate`]。
pub fn generate_bytes(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    match rand::rngs::SysRng.try_fill_bytes(&mut bytes) {
        Ok(()) => hex(&bytes),
        Err(e) => {
            // 走 tracing 而不是 eprintln：守护进程模式下日志要落文件
            tracing::error!("取系统随机数失败，令牌生成中止（不会退回弱随机）: {e}");
            String::new()
        }
    }
}

/// 生成一个 **必定可用** 的令牌，取不到就 panic。
///
/// 只给「没有合理降级路径」的场合用：比如界面服务启动时必须有会话令牌，
/// 否则整个界面都打不开 —— 那种情况下带着一个假令牌继续跑毫无意义。
pub fn generate_or_panic() -> String {
    let t = generate();
    assert!(!t.is_empty(), "系统随机数不可用，无法生成安全令牌");
    t
}

/// 字节转小写十六进制。
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        // 写进 String 不会失败；真失败了也没有合理的补救手段
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_and_charset() {
        let t = generate();
        assert_eq!(t.len(), 32);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(t.chars().all(|c| !c.is_ascii_uppercase()));
    }

    #[test]
    fn is_not_constant() {
        // 同一次运行里连取两次必须不同 —— 这正是旧实现最容易翻车的点：
        // 同一毫秒内 pid 也不变，位混合会给出同样的结果。
        let a = generate();
        let b = generate();
        assert!(!a.is_empty() && !b.is_empty(), "系统随机数不可用");
        assert_ne!(a, b, "两次生成的令牌相同，随机源有问题");
    }

    #[test]
    fn distinct_across_many_calls() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..256 {
            let t = generate();
            assert!(!t.is_empty(), "系统随机数不可用");
            assert!(seen.insert(t), "生成 256 个令牌出现了重复");
        }
    }

    #[test]
    fn custom_length() {
        assert_eq!(generate_bytes(32).len(), 64);
    }

    #[test]
    fn or_panic_returns_usable_token() {
        assert_eq!(generate_or_panic().len(), 32);
    }
}
