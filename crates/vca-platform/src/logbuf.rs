//! 日志环形缓冲：把 `tracing` 的输出留在内存里，供界面读取。
//!
//! # 为什么需要它
//!
//! 程序原本把日志写 stdout（控制台）。GUI 模式下控制台窗口要隐藏，
//! 于是这些输出**没有地方可去** —— 用户点了「开始运行」，后台出了什么事
//! 完全看不见，只能去翻 `logs/ui.log`。日志文件仍然写（排查问题时有用），
//! 但界面里必须有一份实时的。
//!
//! 本模块提供：
//! - 按级别过滤后**追加**到内存环形缓冲（默认 2000 行）；
//! - 超过容量就丢最老的，内存占用有上界（一跑一学期，不能无限涨）；
//! - 支持按游标增量读取，界面据此做"只取新增的那几行"的轮询。
//!
//! # 为什么不解析日志文件
//!
//! 文件是**追加写**的，读取端要处理"文件被轮转""多进程同时写"这些问题；
//! 而且每次轮询都要重读整个文件。内存缓冲只有一个进程、一把锁，简单得多。
//!
//! # 与 `tracing-subscriber` 的关系
//!
//! 本模块是一个 [`tracing_subscriber::Layer`]，叠加在原有的 fmt layer 之上。
//! 原有的控制台/文件输出**一个都不少** —— 只是多了一个去处。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

/// 内存里保留的最大日志行数。
///
/// 2000 行按每行 ~200 字节算是 400 KB 左右 —— 对 8 GB 的机器完全无所谓，
/// 而"最近 2000 行"足以覆盖一次课后流水线的全过程（那大约几百行）。
pub const MAX_LINES: usize = 2000;

/// 一行日志。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    /// 单调递增的序号，界面用它做增量拉取的游标。
    pub seq: u64,
    /// `HH:MM:SS` 形式的本地时间。
    pub time: String,
    /// 级别：`INFO` / `WARN` / `ERROR` / `DEBUG` / `TRACE`。
    pub level: String,
    /// 正文（已把字段拼成 `key=value` 形式，保持可读）。
    pub text: String,
    /// 记录来源（模块路径），界面可用来做筛选。
    pub target: String,
}

impl LogLine {
    /// 拼成一行可读文本，格式与原先的控制台输出一致。
    ///
    /// 保持 `时间 级别 正文` 这个顺序，是因为用户已经习惯在控制台里
    /// 按这个格式扫日志；换到界面里不该让人重新适应。
    pub fn display(&self) -> String {
        format!("{} {:5} {}", self.time, self.level, self.text)
    }

    /// 转成 JSON，供 `/api/logs` 返回。
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "seq": self.seq,
            "time": self.time,
            "level": self.level,
            "text": self.text,
            "target": self.target,
        })
    }
}

/// 全局日志缓冲。
///
/// 用 `OnceLock` 而不是 lazy_static：标准库自带，且初始化是幂等的。
/// 缓冲本身在 `Mutex` 里 —— 日志线程与 HTTP 线程都要碰它。
struct Buffer {
    lines: VecDeque<LogLine>,
    next_seq: u64,
    /// 最近一次清空时的序号，用于让界面知道"你手里的游标过期了"。
    cleared_at: u64,
}

fn buffer() -> &'static Mutex<Buffer> {
    static BUF: OnceLock<Mutex<Buffer>> = OnceLock::new();
    BUF.get_or_init(|| {
        Mutex::new(Buffer {
            lines: VecDeque::with_capacity(MAX_LINES),
            next_seq: 0,
            cleared_at: 0,
        })
    })
}

/// 追加一行日志（供 layer 与测试调用）。
pub fn push(level: &str, target: &str, text: String) {
    let mut b = buffer().lock().unwrap_or_else(|e| e.into_inner());
    let seq = b.next_seq;
    b.next_seq += 1;
    if b.lines.len() >= MAX_LINES {
        // 丢最老的：内存必须有上界，否则跑一个学期会撑爆
        b.lines.pop_front();
    }
    b.lines.push_back(LogLine {
        seq,
        time: now_hms(),
        level: level.to_string(),
        text,
        target: target.to_string(),
    });
}

/// 读取日志。
///
/// - `since_seq`: 只要序号 **大于** 它的行；传 `None` 取最新的 `limit` 行。
/// - `limit`: 最多返回多少行（从尾部截取）。
///
/// 返回 `(行列表, 当前总行数, 是否发生过清空)`。
pub fn snapshot(since_seq: Option<u64>, limit: usize) -> (Vec<LogLine>, usize, bool) {
    let b = buffer().lock().unwrap_or_else(|e| e.into_inner());
    let cleared = since_seq.map(|s| s < b.cleared_at).unwrap_or(false);

    let picked: Vec<LogLine> = match since_seq {
        // 增量模式：拿游标之后的所有新行
        Some(s) => b.lines.iter().filter(|l| l.seq > s).cloned().collect(),
        // 全量模式：取尾部 limit 行
        None => {
            let n = limit.min(b.lines.len());
            b.lines.iter().skip(b.lines.len() - n).cloned().collect()
        }
    };
    // 增量模式下也要有上限：万一界面离线很久，一次别回几十万行
    let picked = if picked.len() > limit {
        picked[picked.len() - limit..].to_vec()
    } else {
        picked
    };
    (picked, b.lines.len(), cleared)
}

/// 清空内存缓冲。
///
/// 只清内存，**不动日志文件** —— 用户点「清空」是想让界面干净点，
/// 不是想把磁盘上的排查记录也抹掉。
pub fn clear() {
    let mut b = buffer().lock().unwrap_or_else(|e| e.into_inner());
    b.lines.clear();
    b.cleared_at = b.next_seq;
}

/// `HH:MM:SS` 本地时间。
///
/// 直接复用 [`crate::clock::now_local_secs`]（内部是 Win32 `GetLocalTime`），
/// 不自己算时区 —— 两套时区逻辑必然漂移，而日志时间戳错了会让人
/// 把因果看反，比没有时间戳更糟。
fn now_hms() -> String {
    let (dt, sec) = crate::clock::now_local_secs();
    format!("{:02}:{:02}:{:02}", dt.hour, dt.minute, sec)
}

/// `tracing` 的 Layer：把每条事件同时塞进内存缓冲。
///
/// 只做这一件事 —— 控制台与文件的输出仍由原有的 fmt layer 负责，
/// 两者互不影响。
pub struct MemoryLayer;

impl<S: Subscriber> Layer<S> for MemoryLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        // 级别过滤交给 tracing 自身的 EnvFilter（在 subscriber 里统一设），
        // 这里只做格式化
        let mut v = FieldVisitor::default();
        event.record(&mut v);
        push(level_str(meta.level()), meta.target(), v.finish());
    }
}

/// 级别名，定长 5 字符（与 `LogLine::display` 的对齐一致）。
fn level_str(l: &Level) -> &'static str {
    match *l {
        Level::ERROR => "ERROR",
        Level::WARN => "WARN",
        Level::INFO => "INFO",
        Level::DEBUG => "DEBUG",
        Level::TRACE => "TRACE",
    }
}

/// 把事件的字段拼成一行文本。
///
/// 规则与 `tracing-subscriber` 默认的 fmt 一致：
/// - 第一个名为 `message` 的字段裸写（它才是人看的正文）；
/// - 其余字段按 `key=value` 跟在后面；
/// - 用空格分隔，不加颜色（界面自己按级别上色）。
#[derive(Default)]
struct FieldVisitor {
    message: String,
    rest: Vec<String>,
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            // `{:?}` 会带引号；message 通常是字符串，去掉更接近控制台观感
            let s = format!("{value:?}");
            self.message = strip_debug_quotes(s);
        } else {
            self.rest.push(format!("{}={:?}", field.name(), value));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.rest.push(format!("{}={}", field.name(), value));
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.rest.push(format!("{}={}", field.name(), value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.rest.push(format!("{}={}", field.name(), value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.rest.push(format!("{}={}", field.name(), value));
    }
}

impl FieldVisitor {
    fn finish(self) -> String {
        if self.rest.is_empty() {
            self.message
        } else if self.message.is_empty() {
            self.rest.join(" ")
        } else {
            format!("{} {}", self.message, self.rest.join(" "))
        }
    }
}

/// 去掉 `Debug` 给字符串加的引号（只处理两端成对的 `"`）。
fn strip_debug_quotes(s: String) -> String {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s[1..s.len() - 1].to_string()
    } else {
        s
    }
}

/// 供 `tracing_subscriber` 注册用：返回可克隆的 layer。
pub fn layer() -> MemoryLayer {
    MemoryLayer
}

/// 一个可跨线程共享的日志写入句柄（供不方便走 tracing 的地方使用）。
pub type SharedSink = Arc<dyn Fn(&str, &str, String) + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;

    /// 串行化这些测试。
    ///
    /// 缓冲是**全局静态**（这正是产品需要的样子：任何线程写的日志都要能被
    /// 界面读到），但 `cargo test` 默认多线程并行 —— 几个测试同时 `clear()`
    /// 再各自 `push`，会互相踩。症状很迷惑：`push_and_snapshot_tail` 断言
    /// 总数 10，实际 40，看起来像取数逻辑错了，其实是别的测试往里塞了行。
    ///
    /// 所以这里用一把测试专用的锁把它们串起来。`into_inner()` 是为了
    /// 前一个测试 panic 之后锁仍然可用，后续测试能照常跑完（否则会连锁失败，
    /// 看不到真正的问题）。
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn push_and_snapshot_tail() {
        let _g = serial();
        clear();
        for i in 0..10 {
            push("INFO", "t", format!("line {i}"));
        }
        let (lines, total, _) = snapshot(None, 5);
        assert_eq!(total, 10);
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[4].text, "line 9", "应取最新的 5 行");
    }

    #[test]
    fn incremental_read_by_cursor() {
        let _g = serial();
        clear();
        push("INFO", "t", "a".into());
        push("INFO", "t", "b".into());
        let (first, _, _) = snapshot(None, 100);
        let cursor = first.last().unwrap().seq;

        push("WARN", "t", "c".into());
        let (newer, _, cleared) = snapshot(Some(cursor), 100);
        assert!(!cleared);
        assert_eq!(newer.len(), 1, "只应拿到游标之后的那一行");
        assert_eq!(newer[0].text, "c");
        assert_eq!(newer[0].level, "WARN");
    }

    #[test]
    fn ring_buffer_has_upper_bound() {
        let _g = serial();
        clear();
        for i in 0..(MAX_LINES + 50) {
            push("INFO", "t", format!("n{i}"));
        }
        let (_, total, _) = snapshot(None, MAX_LINES * 2);
        assert_eq!(total, MAX_LINES, "内存里不能超过上限");
        let (tail, _, _) = snapshot(None, 1);
        assert_eq!(
            tail[0].text,
            format!("n{}", MAX_LINES + 49),
            "被丢掉的应该是最老的"
        );
    }

    #[test]
    fn clear_keeps_monotonic_seq_and_signals_cursor_expiry() {
        let _g = serial();
        clear();
        push("INFO", "t", "x".into());
        let (before, _, _) = snapshot(None, 10);
        let cursor = before.last().unwrap().seq;

        clear();
        let (after, total, cleared) = snapshot(Some(cursor), 10);
        assert!(cleared, "清空后应告诉调用方游标过期了");
        assert_eq!(total, 0);
        assert!(after.is_empty());

        // 序号继续递增，不会与清空前的行冲突
        push("INFO", "t", "y".into());
        let (again, _, _) = snapshot(None, 10);
        assert!(again[0].seq > cursor, "序号必须单调，否则界面会漏行");
    }

    #[test]
    fn display_matches_console_shape() {
        let _g = serial();
        clear();
        push("ERROR", "vca::x", "炸了".into());
        let (lines, _, _) = snapshot(None, 1);
        let d = lines[0].display();
        assert!(d.contains("ERROR"), "应含级别: {d}");
        assert!(d.contains("炸了"), "应含正文: {d}");
        // HH:MM:SS 在最前
        assert_eq!(&d[2..3], ":", "时间应在最前面: {d}");
    }

    #[test]
    fn limit_applies_to_incremental_reads_too() {
        let _g = serial();
        clear();
        for i in 0..500 {
            push("INFO", "t", format!("a{i}"));
        }
        // 游标停在 0，但只允许返回 10 行 —— 不能因为积压就把 500 行全吐出来
        let (lines, _, _) = snapshot(Some(0), 10);
        assert_eq!(lines.len(), 10);
        assert_eq!(lines[9].text, "a499", "截取的应是尾部（最新的）");
    }
}
