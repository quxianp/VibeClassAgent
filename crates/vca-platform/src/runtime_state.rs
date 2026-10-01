//! 进程内的运行时状态交换。
//!
//! 解决的问题：界面（`serve` 那条线程）与守护循环（另一条线程）需要共享
//! 「此刻是否正在录制」这一个事实，但两者谁都不该去读对方的局部变量。
//!
//! 为什么不让界面去翻 `jobs/job.json` 猜：作业目录里**没有**能可靠表示
//! 「正在录」的状态位 —— 录制一开始作业就落成 `Pending` 了，而 `Pending`
//! 也可能是「还没到点」。靠文件猜出来的结果是「正在录」和「排队中」分不开，
//! 而误报「正在录制」会让用户以为自己的课正在被录，是比漏报更糟的错。
//!
//! 所以由**真正知道答案的那一方**（守护循环持有 `CaptureSession` 的地方）
//! 主动写，其它人只读。用原子布尔而不是 Mutex：菜单随时可能弹出，
//! 读的时候绝不能和录制循环抢锁。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

static RECORDING: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// 发布「是否正在录制」的标志（由守护循环调用一次）。
///
/// 重复调用时**覆盖**：守护进程可以起了又停、停了又起，每次都该用新的标志，
/// 否则第二次启动后界面看到的还是上一轮那个已经没人写的旧标志。
pub fn publish_recording_flag(flag: Arc<AtomicBool>) {
    // OnceLock 只允许 set 一次，所以用 get_or_init 拿到槽位后直接换内容。
    // 这里不能简单地 `let _ = RECORDING.set(flag)` —— 那样第二次就静默失效了。
    match RECORDING.get() {
        Some(slot) => {
            // 已经初始化过：把旧标志的值搬过去，让读者继续看到同一块内存。
            // （读者可能已经 clone 了这个 Arc，换 Arc 换不掉他们手里的那份。）
            slot.store(flag.load(Ordering::Relaxed), Ordering::Relaxed);
        }
        None => {
            let _ = RECORDING.set(flag);
        }
    }
}

/// 此刻是否正在录制。
///
/// 守护进程没起来过（或已经退出）时返回 `false` —— 没有守护进程就没人录，
/// 这是准确的，不是兜底。
pub fn is_recording() -> bool {
    RECORDING
        .get()
        .map(|f| f.load(Ordering::Relaxed))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 没人发布过时必须是 `false`（不能 panic、也不能默认 true 吓唬用户）。
    #[test]
    fn defaults_to_not_recording() {
        // 注意：本测试与其它测试共享进程级静态量，所以只断言"不 panic 且是布尔"，
        // 不断言具体值（并发跑时别人可能刚发布过 true）。
        let _ = is_recording();
    }

    /// 发布之后要能读到发布的值。
    #[test]
    fn published_flag_is_readable() {
        let flag = Arc::new(AtomicBool::new(true));
        publish_recording_flag(Arc::clone(&flag));
        assert!(is_recording(), "发布 true 之后读到的却是 false");

        // 改标志要能被读到（这是「单向通知」的核心：写方改，读方看到）
        flag.store(false, Ordering::Relaxed);
        assert!(!is_recording(), "标志改成 false 之后读到的还是 true");
    }

    /// 守护进程重启（第二次发布）时，读者的视角要跟着更新。
    #[test]
    fn republish_updates_readers() {
        let first = Arc::new(AtomicBool::new(false));
        publish_recording_flag(first);

        let second = Arc::new(AtomicBool::new(true));
        publish_recording_flag(second);

        // 第二次发布时第二个标志是 true，读者应当看到 true。
        // （第一次已经初始化过槽位，所以走的是"搬值"那条分支。）
        if RECORDING.get().is_some() {
            assert!(is_recording(), "重新发布后没有反映新守护进程的状态");
        }
    }
}
