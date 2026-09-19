# Rust 上手文档（面向有 C++ / Python 基础者）

> 目标读者：学过 C++（含算法竞赛）或 Python，没系统写过 Rust
> 特点：**每个概念都指向本项目里的真实代码**，边读边对照
> 建议：先读 14，再动手改一处小代码，然后按 9 的练习路径推进

---

## 目录

1. [心智模型：Rust 想解决什么问题](#1-心智模型rust-想解决什么问题)
2. [从 C++/Python 迁移对照表](#2-从-cpython-迁移对照表)
3. [所有权与借用（最核心）](#3-所有权与借用最核心)
4. [错误处理：Result 与 ?](#4-错误处理result-与-)
5. [枚举与模式匹配](#5-枚举与模式匹配)
6. [trait 与泛型](#6-trait-与泛型)
7. [模块与 crate](#7-模块与-crate)
8. [并发与线程](#8-并发与线程)
9. [本项目实战索引](#9-本项目实战索引)
10. [常见编译错误速查](#10-常见编译错误速查)
11. [工具链与日常命令](#11-工具链与日常命令)
12. [练习路径](#12-练习路径)

---

## 1. 心智模型：Rust 想解决什么问题

C++ 给你**完全的控制权**，代价是内存错误、悬垂指针、数据竞争全靠人盯。
Python 给你**安全与便利**，代价是运行时开销与 GIL，且类型错误要到运行时才暴露。

Rust 的定位是：**编译期就把这两类错误挡掉，同时不引入 GC**。

三个核心机制：

| 机制 | 挡掉什么 | 代价 |
|---|---|---|
| 所有权 / 借用 | 悬垂指针、重复释放、数据竞争 | 需要理解借用规则，写代码时要「顺」着编译器 |
| `Result` / `Option` | 空指针、未处理的错误 | 每个可能失败的地方都要显式处理 |
| trait 约束 | 类型不匹配、接口不一致 | 需要设计好抽象 |

**一个心态建议**：别把 Rust 编译器当敌人。它报错的地方，在 C++ 里就是半夜崩溃的地方。
本项目里 `overlay.rs` 反复出问题（文字不显示、圆角锯齿）时，Rust 编译器一次都没让我踩到内存坑
所有问题都出在 Win32 API 用法上。

---

## 2. 从 C++/Python 迁移对照表

| 概念 | C++ | Python | Rust |
|---|---|---|---|
| 变量默认 | 可变 | 可变 | **不可变**（要 `mut`） |
| 类型 | 静态 | 动态 | 静态 + **强推导**（`let x = 1;`） |
| 内存释放 | 手动 / RAII | 引用计数 + GC | **作用域结束自动 drop**（编译期确定） |
| 空值 | `nullptr` | `None` | **没有 null**，用 `Option<T>` |
| 错误 | 异常 / 错误码 | 异常 | **`Result<T, E>` + `?`** |
| 多态 | 虚函数 | 鸭子类型 | trait（静态分发）或 `dyn`（动态分发） |
| 泛型 | 模板（编译期展开） | 无 | 泛型 + trait 约束（单态化） |
| 字符串 | `std::string` | `str` | `String`（拥有）/ `&str`（借用） |
| 数组 | `std::vector` | `list` | `Vec<T>` |
| 哈希表 | `unordered_map` | `dict` | `HashMap<K,V>` |
| 接口 | 抽象类 | 抽象基类 | `trait` |
| 包管理 | vcpkg / CMake | pip | **Cargo**（内建） |
| 测试 | 第三方框架 | pytest | **内建**（`#[test]`） |
| 文档 | Doxygen | docstring | **内建**（`///` + `cargo doc`） |

---

## 3. 所有权与借用（最核心）

### 3.1 三条规则

1. 每个值有**唯一**的所有者；
2. 同一时刻，要么有**多个不可变借用**（`&T`），要么有**一个可变借用**（`&mut T`），不能并存；
3. 所有者离开作用域，值被 `drop`。

### 3.2 移动（move）

```rust
let s1 = String::from("hello");
let s2 = s1;            // s1 被「移动」到 s2
println!("{s1}");       //  编译错误：s1 已被移动
```

C++ 里 `s2 = s1` 是拷贝（或移动构造），Python 里是两个名字指向同一对象。
Rust 默认是「移动 + 原变量失效」，从语言层面杜绝 double free。

**想拷贝就显式写 `.clone()`**：

```rust
let s2 = s1.clone();    // 深拷贝，两者都可用
```

> 本项目里 `OverlayStyle` 加了 `#[derive(Clone)]`，因为窗口线程需要拿到一份自己的样式副本。
> 见 `crates/vca-platform/src/overlay.rs`。

### 3.3 借用（borrow）

```rust
fn len_of(s: &str) -> usize { s.len() }     // 借用，不夺走所有权

let s = String::from("hi");
let n = len_of(&s);                          // 传引用
println!("{s} {n}");                         //  s 还能用
```

**可变借用是独占的**：

```rust
let mut v = vec![1, 2, 3];
let a = &v[0];
v.push(4);              //  编译错误：v 已被不可变借用
println!("{a}");
```

这是 Rust 防数据竞争的核心。在 C++ 里这会导致迭代器失效（运行时崩溃），Rust 直接不让编译。

> **本项目实例**：`crates/vca-platform/src/overlay.rs` 里，窗口样式存在
> `static STYLE: Mutex<Option<OverlayStyle>>` 中，每次读取时 `lock()` 拿一份克隆，
> 而不是长期持有引用就是为了避免跨线程的借用冲突。

### 3.4 生命周期（先会用再学懂）

大多数时候编译器能自动推断，你不需要写。只有一种常见场景需要标注：

```rust
// 返回值的生命周期跟哪个参数走？
fn longest<'a>(x: &'a str, y: &'a str) -> &'a str {
    if x.len() > y.len() { x } else { y }
}
```

**实用建议**：刚开始遇到生命周期报错，优先想「我是不是想返回一个局部变量的引用」
99% 的情况是这个问题，解法是返回拥有所有权的值（`String` 而非 `&str`）。

---

## 4. 错误处理：Result 与 ?

### 4.1 基本形态

```rust
use std::fs;

fn read_config(path: &str) -> Result<String, std::io::Error> {
    fs::read_to_string(path)          // 返回 Result<String, io::Error>
}
```

`Result<T, E>` 是一个枚举：

```rust
enum Result<T, E> {
    Ok(T),
    Err(E),
}
```

### 4.2 `?` 运算符：错误的自动传播

```rust
fn load(path: &str) -> Result<Settings, LoadError> {
    let text = fs::read_to_string(path)
        .map_err(|e| LoadError::Io { path: path.into(), source: e })?;  //  失败就返回
    let s = serde_yaml::from_str(&text)?;                                //  同上
    Ok(s)
}
```

对比：
- C++：`if (err) return err;` 或异常
- Python：`raise` / 不处理就崩
- Rust：`?` 一行搞定，且**类型系统保证你不会忘记处理**

> **本项目实例**：`crates/vca-core/src/config.rs` 的 `load_settings` / `load_schedule_file`，
> 以及 `crates/vca-core/src/import.rs` 的 `import_classisland`。

### 4.3 `Option<T>`：没有 null

```rust
let mut data: Option<String> = None;

// 方式一：if let
if let Some(v) = data {
    println!("{v}");
}

// 方式二：提供默认值
let v = data.unwrap_or_default();

// 方式三：链式
let len = data.as_ref().map(|s| s.len()).unwrap_or(0);
```

`unwrap()` / `expect()` 会在 `None`/`Err` 时 **panic**，只应在「逻辑上不可能失败」时用。

> **本项目实例**：`crates/vca-core/src/time.rs` 的 `parse_hhmm` 返回 `Option<u32>`，
> 因为「解析失败」是正常情况（用户可能填错），不该 panic。

### 4.4 自定义错误类型

用 `thiserror` 声明，几乎零样板：

```rust
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("读取文件失败 {path}: {source}")]
    Io { path: String, source: std::io::Error },

    #[error("解析 YAML 失败 {path}: {source}")]
    Yaml { path: String, source: serde_yaml::Error },
}
```

> **本项目实例**：`crates/vca-core/src/import.rs::ImportError`、
> `crates/vca-platform/src/http.rs::HttpError`、`crates/vca-plugin-host/src/market.rs::MarketError`。

---

## 5. 枚举与模式匹配

Rust 的 `enum` 是**代数数据类型**，每个变体可以带不同数据：

```rust
pub enum JobState {
    Pending,
    Recorded,
    Transcribed,
    Pushed,
    Failed,
}
```

`match` 必须**穷尽所有分支**（编译器强制）：

```rust
match job.state {
    JobState::Pushed => { /* 登记清理计划 */ }
    JobState::Failed => { /* 记录错误 */ }
    other => tracing::info!("跳过 {other:?}"),
}
```

**为什么这很重要**：以后你给 `JobState` 加一个新状态，编译器会**列出所有需要改的地方**。
这是大型项目长期可维护的关键。

> **本项目实例**：`crates/vca-core/src/job.rs`。
> `JobState::can_transition_to` 用 `matches!` 宏把合法迁移列成一张表，
> 非法迁移会返回 `CoreError::InvalidTransition`状态机的正确性由代码而非注释保证。

---

## 6. trait 与泛型

### 6.1 trait  接口

```rust
pub trait Pusher {
    fn provider(&self) -> Provider;
    fn send(&self, doc: &PushDoc) -> Result<PushOutcome, PushError>;
}
```

实现：

```rust
impl Pusher for WeComPusher {
    fn provider(&self) -> Provider { Provider::WeCom }
    fn send(&self, doc: &PushDoc) -> Result<PushOutcome, PushError> {
        // ...
    }
}
```

> **本项目实例**：`crates/vca-platform/src/push.rs`。
> 企业微信 / Webhook / Server 酱 / dry-run 四个渠道实现同一个 trait，
> 调用方只依赖 trait 对象 `Box<dyn Pusher>`，加渠道不用改调用代码。

### 6.2 静态分发 vs 动态分发

```rust
fn send_static<P: Pusher>(p: &P) { }        // 单态化，零开销，代码膨胀
fn send_dyn(p: &dyn Pusher) { }             // 虚表，一次间接跳转，体积小
```

本项目用 `Box<dyn Pusher>`，因为渠道数量少、调用不频繁，体积更重要。

### 6.3 `#[derive(...)]` 自动实现

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct LessonSummary { /* ... */ }
```

一行得到：调试打印、克隆、比较、JSON 序列化/反序列化、默认值。
这相当于 C++ 里手写一堆运算符重载，或 Python 里的 `dataclass`。

---

## 7. 模块与 crate

| 层级 | 对应 C++ | 对应 Python | 本项目 |
|---|---|---|---|
| crate | 一个库/可执行体 | 一个包 | `vca-core`、`vca-cli` |
| module | namespace | module | `vca_core::time`、`vca_core::schedule` |
| workspace | solution | 多包仓库 | 根 `Cargo.toml` 聚合 6 个 crate |

**依赖方向必须单向**（本项目的硬规则）：

```
vca-cli > vca-core       （纯逻辑，无 IO、无 unsafe）
          > vca-ipc        （协议契约）
          > vca-plugin-host（插件宿主）
          > vca-platform   （唯一允许 unsafe 的地方）
          > vca-engine     （编排）
```

`core` 与 `ipc` **不反向依赖任何上层**，所以可以脱离 Windows 单测。

---

## 8. 并发与线程

### 8.1 起线程

```rust
let handle = std::thread::Builder::new()
    .name("vca-overlay".to_string())
    .spawn(move || {
        // 线程体
    })?;
```

`move` 关键字把闭包捕获的变量**移动**进线程这样编译器能保证没有悬垂引用。

> **本项目实例**：`overlay.rs` 的 `spawn()` 把窗口样式 move 进窗口线程。

### 8.2 Send / Sync

- `Send`：可以安全地移动到另一个线程；
- `Sync`：可以安全地被多个线程共享引用。

编译器自动推导，不满足就不让编译。**Win32 的窗口句柄（`HWND`）不是 `Send`**，
所以本项目里把它存成 `isize` 再加自己的 `Overlay` 包装（见 `overlay.rs` 的 `Overlay` 结构）。

### 8.3 共享状态

```rust
static STYLE: std::sync::Mutex<Option<OverlayStyle>> = std::sync::Mutex::new(None);

// 写
if let Ok(mut g) = STYLE.lock() { *g = Some(style); }

// 读（克隆一份出来，避免长期持锁）
let s = STYLE.lock().ok().and_then(|g| g.clone());
```

**Rust 的 `Mutex` 把「数据」和「锁」绑在一起**，你不 lock 就拿不到数据
从根上防止忘记加锁。这点比 C++ 的 `std::mutex` 安全得多。

---

## 9. 本项目实战索引

想通过读代码学 Rust，按这个顺序：

| 顺序 | 文件 | 学到什么 |
|---|---|---|
| 1 | `crates/vca-core/src/time.rs` | 纯函数、`Option`、`Display`、内建测试 `#[cfg(test)]` |
| 2 | `crates/vca-core/src/job.rs` | `enum` + `match` + 自定义错误 |
| 3 | `crates/vca-core/src/schedule.rs` | 迭代器链、切片、`Vec` 操作、13 个测试 |
| 4 | `crates/vca-core/src/config.rs` | serde 派生、`?` 传播、文件 IO |
| 5 | `crates/vca-core/src/import.rs` | `serde_json::Value` 动态解析、错误类型设计 |
| 6 | `crates/vca-platform/src/push.rs` | trait + trait 对象 + 多实现 |
| 7 | `crates/vca-platform/src/http.rs` | **FFI**（`extern "system"`）、`unsafe` 块、指针操作 |
| 8 | `crates/vca-platform/src/overlay.rs` | 线程 + `Mutex` + 逐像素渲染 + 消息循环 |
| 9 | `crates/vca-engine/src/pipeline.rs` | 状态推进、错误降级、模块协作 |
| 10 | `crates/vca-engine/src/daemon.rs` | 长驻循环、时间计算、资源生命周期 |

### 9.1 一个完整的小例子（来自本项目）

```rust
// crates/vca-core/src/time.rs
impl LocalDateTime {
    /// 解析 `HH:mm` 为「当日第几分钟」。
    pub fn parse_hhmm(s: &str) -> Option<u32> {
        let (h, m) = s.trim().split_once(':')?;   // ? 在 Option 上也能用
        let h: u32 = h.trim().parse().ok()?;      // ok() 把 Result 转 Option
        let m: u32 = m.trim().parse().ok()?;
        if h > 23 || m > 59 {
            return None;
        }
        Some(h * 60 + m)
    }
}
```

对照 Python：

```python
def parse_hhmm(s: str) -> int | None:
    try:
        h, m = s.strip().split(":", 1)
        h, m = int(h.strip()), int(m.strip())
    except ValueError:
        return None
    if h > 23 or m > 59:
        return None
    return h * 60 + m
```

Python 用异常做控制流；Rust 用 `Option` + `?`，编译器强制你在调用处处理 `None`。

---

## 10. 常见编译错误速查

| 错误 | 含义 | 解法 |
|---|---|---|
| `value moved here` | 值已被移动 | 用 `.clone()`，或改成传引用 `&` |
| `cannot borrow as mutable` | 已有不可变借用 | 缩小借用作用域，或先 `clone()` |
| `does not live long enough` | 引用了局部变量 | 返回拥有所有权的类型（`String` 而非 `&str`） |
| `no method named X found` | 缺 trait 导入 | 在文件顶部 `use` 对应 trait |
| `the trait bound X is not satisfied` | 类型没实现所需 trait | 加 `#[derive(...)]` 或手写 `impl` |
| `cannot find type X in this scope` | 缺导入 | `use crate::xxx::X;` |
| `mismatched types` | 类型不符 | 看报错的 `expected` / `found`，用 `as` 或 `into()` |
| `unused variable` | 变量没用 | 加下划线 `_x` 或删掉 |
| `cannot return value referencing local variable` | 返回了局部引用 | 返回拥有所有权的值 |

**实用技巧**：报错信息里的 `help:` 通常直接给你可复制的修复代码。
`cargo clippy` 比编译器更严格，能指出「能写得更地道」的地方。

---

## 11. 工具链与日常命令

```bash
cargo new myproj          # 新建项目
cargo build               # 调试构建
cargo build --release     # 发布构建
cargo run                 # 构建并运行
cargo test                # 跑测试
cargo fmt                 # 格式化（提交前必跑）
cargo clippy              # 静态检查（比编译器严）
cargo doc --open          # 生成并打开文档
cargo add serde           # 加依赖（需要 cargo-edit）
cargo tree                # 看依赖树
cargo update              # 更新到兼容范围内最新版
```

**本项目**在受限环境下的等价命令：

```bat
scripts\dev.cmd doctor          :: 或直接跑脚本，它会自动设置好工具链环境
scripts\dev.cmd --help          :: 透传给 vca 的参数
```

或手动：

```powershell
$env:CARGO_HOME  = "D:\VibeClassAgent\.toolchain\cargo"
$env:RUSTUP_HOME = "D:\VibeClassAgent\.toolchain\rustup"
$env:PATH = "D:\VibeClassAgent\.toolchain\cargo\bin;$env:PATH"
cargo test
```

---

## 12. 练习路径

按难度递进，每一步都能用 `cargo test` 立刻验证：

### 第 1 关：读懂并修改一个纯函数

打开 `crates/vca-core/src/time.rs`，把 `parse_hhmm` 改成**也接受 `H:mm` 之外的空格**（其实已支持）。
再试着加一个 `parse_hhmm_secs` 返回秒数，并补一个 `#[test]`。

```rust
#[test]
fn my_new_test() {
    assert_eq!(LocalDateTime::parse_hhmm("08:00"), Some(480));
}
```

### 第 2 关：加一个枚举分支

在 `crates/vca-core/src/job.rs` 的 `JobState` 加一个 `Archived` 状态，
然后修好编译器指出的**所有** `match`你会立刻体会到穷尽匹配的价值。

### 第 3 关：实现一个 trait

在 `crates/vca-platform/src/push.rs` 里照着 `WebhookPusher` 加一个 `FeishuPusher`，
在 `Provider` 里加分支，在 `make_pusher` 里注册。

### 第 4 关：写一个 CLI 子命令

1. `crates/vca-cli/src/main.rs` 的 `enum Command` 加变体；
2. `main()` 的 `match` 里分发；
3. `cmd.rs` 写实现。

### 第 5 关：读懂 unsafe

打开 `crates/vca-platform/src/overlay.rs`，找到 `render_layered`。
它的 `unsafe` 块里的每一步都有 `// SAFETY:` 注释说明「为什么这里安全」
这是 Rust 项目里 `unsafe` 的正确用法：**每个 unsafe 块都要能自证安全**。

---

## 附录：本项目遵守的编码规范

| 规范 | 位置 | 原因 |
|---|---|---|
| 核心层 `#![forbid(unsafe_code)]` | `vca-core`、`vca-ipc`、`vca-engine` | 纯逻辑，不该有 unsafe |
| 只有 `vca-platform` 用 unsafe | `crates/vca-platform` | 集中审计，FFI 全在一处 |
| 公开项必须有文档 | 各 crate 的 `#![warn(missing_docs)]` | `cargo doc` 可直接交付 |
| 未实现的函数返回明确错误 | 如 `PluginHost::discover` 早期版本 | 不留「看起来能用其实没接」的坑 |
| 提交前跑 fmt + clippy + test | 见 `README.md` | 保持零告警基线 |
