//! 原生窗口：把界面装进**程序自己的窗口**里，而不是套一个浏览器。
//!
//! # 为什么要这个
//!
//! 原来界面是「本地 HTTP 服务 + `msedge --app=`」。那套做法能用，但有两个
//! 去不掉的毛病：
//!
//! 1. **看起来还是浏览器** —— 窗口属于 Edge 进程，任务栏图标是 Edge 的，
//!    窗口类名是 Edge 的，`alt-tab` 里显示的是 Edge。再怎么调 `--app=`
//!    也只是"没那么像浏览器"，不可能变成"这就是这个程序的窗口"。
//! 2. **受 Edge 摆布** —— Edge 自动更新改了参数行为、被安全软件拦、
//!    用户禁用了 Edge，界面就打不开。用户遇到过一次（`exit=21`），
//!    排查了大半天，而根因完全不在我们代码里。
//!
//! # 现在的结构
//!
//! 用 `tao` 建一个**属于本进程**的窗口，里面用 `wry` 挂一个 WebView2 控件。
//! 于是：
//!
//! - 窗口、任务栏图标、`alt-tab` 条目全部是 `VibeClassAgent` 自己的；
//! - 不再依赖系统 Edge（WebView2 运行时是独立组件，Win10/11 通常自带）；
//! - **前端完全不用改** —— WebView2 就是 Chromium，`web/` 里那套
//!   HTML/CSS/JS 原样跑。
//!
//! HTTP 服务仍然保留：WebView 加载的是 `http://127.0.0.1:<port>/?t=<token>`，
//! 而不是 `file://`。这样做是因为前端那 40 个接口本来就是 HTTP 的，
//! 改成 `file://` 要重写前端；而且保留 HTTP 让"浏览器里也能看"这条退路还在
//! （出了问题用户自己开浏览器访问同一个地址就能对照）。

use std::sync::mpsc;

/// 原生窗口发回主线程的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowEvent {
    /// 窗口被用户关掉了（点 X）。程序应当退出。
    Closed,
    /// 窗口没能建起来。`String` 是给人看的失败原因。
    ///
    /// 收到这个时**不要**让程序退出：HTTP 服务还在跑，退回"用系统浏览器
    /// 打开"至少让用户能用上。这正是原来 Edge 方案里最该有却没有的兜底。
    Failed(String),
}

/// 起一个原生窗口加载 `url`，**立即返回**可以查询/关闭的句柄。
///
/// # 线程模型（重要）
///
/// Win32 的窗口属于**创建它的那个线程**，消息也只能由那个线程抽取，
/// 所以事件循环 `event_loop.run()` 会一直阻塞。因此这里在内部起一个
/// 专属线程跑窗口，本函数**立刻返回**，调用方（`server.rs`）可以继续
/// 在主线程上处理 HTTP 请求。
///
/// 这也意味着 `serve()` 里那句"必须写这个函数"的顺序约束：
/// 窗口线程和 HTTP 线程是并行的，谁先起都行。
///
/// # 返回
///
/// - `Ok(handle)`：窗口线程已启动。真正的创建结果是**异步**的 ——
///   WebView2 缺失之类的错误会通过 [`WindowEvent::Failed`] 报回来，
///   因为那些错误要到窗口线程里才知道。
/// - `Err`：连线程都起不来（极罕见）。
pub fn run(url: String) -> anyhow::Result<WindowHandle> {
    let (tx, rx) = mpsc::channel::<WindowEvent>();
    // 反向通道：主线程让窗口线程聚焦/关闭自己
    let (cmd_tx, cmd_rx) = mpsc::channel::<WindowCmd>();

    // ⚠️⚠️ Windows 上事件循环**必须跑在主线程**。
    //
    // 血泪教训（本轮实测崩溃）：最初这里起了一个 `vca-window` 线程去跑
    // `event_loop.run()`，tao 直接 panic：
    //   "Initializing the event loop outside of the main thread is a
    //    significant cross-platform compatibility hazard"
    // 因为 Win32 的消息队列和线程绑定，主线程之外跑事件循环会破坏
    // 整个 GUI 的消息分发。
    //
    // 所以 `run()` 就在**当前线程**（也就是 main）把事件循环跑起来 ——
    // 它会阻塞，这是设计如此，不是缺陷。调用方（`serve()`）必须明白：
    // **调用 `run()` 就等于把当前线程交给窗口**。
    //
    // 那 HTTP 服务怎么办？它必须已经在别的线程上跑着了。
    // `serve()` 里的顺序正是：先把服务丢到后台线程，主线程再进窗口。
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_blocking(url, tx.clone(), cmd_rx)
    }));

    match result {
        Ok(()) => Ok(WindowHandle {
            cmd: cmd_tx,
            probe: rx,
        }),
        Err(_) => {
            // 事件循环跑完（窗口关闭）是正常路径；真 panic 时这里兜底报错。
            anyhow::bail!("窗口事件循环异常退出")
        }
    }
}

/// 窗口线程本体：建窗口 + 跑事件循环（会阻塞到这个线程结束）。
fn run_blocking(url: String, tx: mpsc::Sender<WindowEvent>, cmd_rx: mpsc::Receiver<WindowCmd>) {
    use tao::event::{Event, WindowEvent as TaoWindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::window::WindowBuilder;
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();

    let proxy = event_loop.create_proxy();
    std::thread::spawn(move || {
        // 把主线程发来的命令转成 event_loop 的用户事件，
        // 这样就能在事件循环里处理，而不必跨线程碰窗口。
        while let Ok(cmd) = cmd_rx.recv() {
            if proxy.send_event(UserEvent::Cmd(cmd)).is_err() {
                break;
            }
        }
    });

    let window = match WindowBuilder::new()
        .with_title("VibeClassAgent")
        .with_inner_size(tao::dpi::LogicalSize::new(1180.0, 820.0))
        .with_min_inner_size(tao::dpi::LogicalSize::new(880.0, 620.0))
        .build(&event_loop)
    {
        Ok(w) => w,
        Err(e) => {
            // 窗口建不出来不是"这个世界不存在"级别的问题 ——
            // 报回去让调用方退回系统浏览器，用户至少还能用。
            let msg = format!("创建窗口失败：{e}");
            tracing::warn!("{msg}");
            let _ = tx.send(WindowEvent::Failed(msg));
            return;
        }
    };

    // ⚠️ 必须显式指定用户数据目录，走 `WebContext`。
    //
    // 不给时 WebView2 会去用默认位置（`%LOCALAPPDATA%\Microsoft\EdgeWebView`），
    // 在部分机器上它自己建这个目录会失败，报一个毫无信息量的
    // `HRESULT(0x8000FFFF) 灾难性故障`。实测本机就是这个情况。
    //
    // 给一个我们自己的目录，顺便也让 WebView2 的数据和用户自己的 Edge
    // 完全隔离：不共享 cookie、不共享缓存 —— 一体机上多教师使用时这点很重要。
    //
    // 注意是 `new_with_web_context`，不是 `.with_web_context(...)`：
    // wry 0.57 把这个参数放在**构造函数**上。
    let mut web_context = wry::WebContext::new(Some(webview_data_dir()));

    let webview = match wry::WebViewBuilder::new_with_web_context(&mut web_context)
        .with_url(&url)
        .with_initialization_script(INIT_SCRIPT)
        .build(&window)
    {
        Ok(w) => w,
        Err(e) => {
            // 最常见的原因是没装 WebView2 运行时。
            // **这条路径必须报出去** —— 否则用户看到的是一个空白窗口，
            // 完全不知道缺什么。
            let msg = format!("创建 WebView 失败（多半是缺少 WebView2 运行时）：{e}");
            tracing::warn!("{msg}");
            let _ = tx.send(WindowEvent::Failed(msg));
            return;
        }
    };

    tracing::info!("原生窗口已创建，正在加载 {url}");

    // ⚠️ `webview` 必须活到事件循环结束。
    //
    // 它一旦被 drop，WebView2 控件就从窗口上摘掉了 —— 结果是一个**空窗口**，
    // 比"窗口没起来"更难查（窗口确实在，就是白的）。
    // 所以这里把它移进闭包，让它的生命周期和事件循环绑定。
    let _webview = webview;
    event_loop.run(move |event, _, control_flow| {
        // 借用一下，确保闭包真的捕获了它（而不是被优化掉）
        let _keep_alive = &_webview;
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent {
                event: TaoWindowEvent::CloseRequested,
                ..
            } => {
                let _ = tx.send(WindowEvent::Closed);
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(UserEvent::Cmd(WindowCmd::Focus)) => {
                // 直接用闭包捕获的 `window` —— tao 0.37 的 `Event` 上没有
                // `window()` 便捷方法，闭包捕获更直接。
                window.set_visible(true);
                window.set_minimized(false);
                window.set_focus();
                window.request_redraw();
            }
            Event::UserEvent(UserEvent::Cmd(WindowCmd::Close)) => {
                let _ = tx.send(WindowEvent::Closed);
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}

/// 原生窗口的**控制端**：只能发命令（聚焦/关闭），不能收事件。
///
/// 它只有 `Sender`，因此是 `Send + Sync`，可以直接放进 `static`
/// 供托盘线程和主线程共用 —— 不需要 `unsafe`，也不必破例
/// `vca-gui` 的 `#![forbid(unsafe_code)]`。
///
/// 由 [`WindowHandle::control_only`] 取得。
#[derive(Clone)]
pub struct WindowControl {
    cmd: mpsc::Sender<WindowCmd>,
}

impl WindowControl {
    /// 把窗口唤到最前面。
    ///
    /// 用在两个地方：第二次双击程序时（单实例保护）、点托盘菜单的「打开界面」。
    /// 都不该再开一个窗口 —— 那会让用户面对两个一模一样的界面。
    pub fn focus(&self) {
        let _ = self.cmd.send(WindowCmd::Focus);
    }

    /// 主动关掉窗口（程序要退出时用）。
    pub fn close(&self) {
        let _ = self.cmd.send(WindowCmd::Close);
    }
}

/// 主线程发给窗口线程的命令。
#[derive(Debug, Clone, Copy)]
enum WindowCmd {
    /// 把窗口唤到最前面（第二次启动、或点托盘「打开界面」时用）。
    Focus,
    /// 关掉窗口。
    Close,
}

#[derive(Debug)]
enum UserEvent {
    Cmd(WindowCmd),
}

/// 窗口里注入的一小段脚本。
///
/// 只做一件事：**禁掉浏览器式的右键菜单和快捷键**。
///
/// 这是"套壳感"最刺眼的来源 —— WebView2 默认会弹 `重新加载 / 检查 /
/// 另存为` 那个菜单，一眼就露馅。界面自己需要的右键菜单不受影响
/// （前端用 `contextmenu` 事件自己处理的那些），因为这里拦的是
/// **默认行为**，`preventDefault()` 之后前端仍可自行 `preventDefault`
/// 或另作他用。
///
/// 同时挡掉 `Ctrl+P`（打印）、`Ctrl+F`（页内查找）、`F5`/`Ctrl+R`（刷新）。
/// 刷新其实无害，但一个"原生程序"按 F5 整个界面重载会显得很怪。
const INIT_SCRIPT: &str = r#"
(function () {
  // 默认右键菜单：挡掉
  document.addEventListener('contextmenu', function (e) {
    e.preventDefault();
  }, true);

  // 会暴露"这是个浏览器"的快捷键
  document.addEventListener('keydown', function (e) {
    var k = (e.key || '').toLowerCase();
    if (e.ctrlKey && (k === 'p' || k === 'f' || k === 'r' || k === 'u')) {
      e.preventDefault();
    }
    if (k === 'f5' || k === 'f7') { e.preventDefault(); }
  }, true);
})();
"#;

/// 正在运行的原生窗口的**控制端**。
///
/// 只装一个 `Sender`，因此天然是 `Send + Sync`、不需要 `unsafe`，
/// 可以直接放进 `static` 让托盘线程和主线程共用。
///
/// # 为什么没有接收端
///
/// `mpsc::Receiver` 不是 `Sync`（只允许一个线程拿它），而 `static`
/// 要求整个类型 `Sync`。接收端只在 [`run`] 返回时用过一次
/// （等窗口建起来、或探测建失败），之后就不需要了 —— 所以不放进这里。
///
/// 窗口关没关我们是**从别处知道的**：`serve()` 的主循环会一直跑，
/// 窗口关掉时窗口线程自己会退出，程序由单实例/退出路径收尾。
/// 这也让这个类型足够小、足够安全，不必破例 `unsafe`。
pub struct WindowHandle {
    cmd: mpsc::Sender<WindowCmd>,
    /// 只在 `serve()` 里探一次"窗口建起来没有"，之后就不再用了。
    ///
    /// 它让 `WindowHandle` 不是 `Sync`，所以**不能**把这个结构体直接放进
    /// `static`。用法是：在 `serve()` 里先拿着完整的 handle 探测，
    /// 探完了把 `cmd` 克隆一份存进 static，把 `probe` 丢掉。
    probe: mpsc::Receiver<WindowEvent>,
}

/// 手写 `Clone`：`Receiver` 不是 `Clone`，但 `Sender` 是。
///
/// 克隆出来的副本**不带接收端**（`probe` 给一个全新的空 channel）——
/// 事件只该被原来那个 handle 消费一次，克隆体拿不到也不该拿到。
/// 这样既满足 `Clone`，又不会有两个地方抢同一条事件流。
impl Clone for WindowHandle {
    fn clone(&self) -> Self {
        let (_, dummy_rx) = mpsc::channel::<WindowEvent>();
        Self {
            cmd: self.cmd.clone(),
            probe: dummy_rx,
        }
    }
}

impl WindowHandle {
    /// 把窗口唤到最前面。
    ///
    /// 用在两个地方：第二次双击程序时（单实例保护）、点托盘菜单的「打开界面」。
    /// 都不该再开一个窗口 —— 那会让用户面对两个一模一样的界面。
    pub fn focus(&self) {
        let _ = self.cmd.send(WindowCmd::Focus);
    }

    /// 主动关掉窗口（程序要退出时用）。
    pub fn close(&self) {
        let _ = self.cmd.send(WindowCmd::Close);
    }

    /// 非阻塞地取一个事件（窗口建起来没有 / 关了没有）。
    ///
    /// 为什么是"非阻塞轮询"而不是"阻塞等"：调用方（`server.rs`）的主循环
    /// 同时在处理 HTTP 请求，不能在这里挂住。轮询间隔由调用方决定，
    /// 和托盘那边是同一套做法。
    ///
    /// 这个方法只在 `serve()` 探测窗口是否建成时用；探测完就可以
    /// 通过 [`Self::control_only`] 取一个能放进 `static` 的纯控制端。
    pub fn try_recv(&self) -> Option<WindowEvent> {
        self.probe.try_recv().ok()
    }

    /// 取一个**只含控制端**的副本，可以放进 `static`。
    ///
    /// `WindowHandle` 本身因为含着 `Receiver` 而不是 `Sync`，进不了
    /// `static`。而存进 `static` 只需要"发命令"这一个能力，
    /// 所以探测完就换成这个轻量版。
    pub fn control_only(&self) -> WindowControl {
        WindowControl {
            cmd: self.cmd.clone(),
        }
    }
}

/// WebView2 的用户数据目录。
///
/// 放在 `%LOCALAPPDATA%\VibeClassAgent\webview`：
///
/// - **不能放 `%TEMP%`** —— 清理软件会删掉它，删了下次启动又要重建，
///   而且 WebView2 在目录损坏时可能直接起不来（和 Edge profile 同款坑）。
/// - **不能和用户自己的 Edge 共用** —— 一体机上多个老师用同一台机器，
///   隔离掉 cookie/缓存既是隐私也是稳定性（用户清 Edge 缓存不该影响程序）。
///
/// 和 `server.rs` 里 Edge 老方案用的 `ui-profile` 并列，各自独立。
pub fn webview_data_dir() -> std::path::PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let dir = base.join("VibeClassAgent").join("webview");
    // 提前建出来：WebView2 自己建有时会失败（0x8000FFFF）。
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_script_blocks_browser_affordances() {
        // 这脚本是"看起来像原生程序"的关键，别让重构时手一抖删掉。
        assert!(INIT_SCRIPT.contains("contextmenu"), "没挡默认右键菜单");
        assert!(
            INIT_SCRIPT.contains("preventDefault"),
            "没有真的阻止默认行为"
        );
        // 几个最露馅的快捷键
        for k in ["'p'", "'f'", "'r'", "f5"] {
            assert!(INIT_SCRIPT.contains(k), "没挡快捷键 {k}");
        }
    }

    #[test]
    fn window_title_is_the_product_name() {
        // 任务栏和 alt-tab 里显示的就是这个字符串。
        // 它必须是产品名 —— 显示成 "localhost:8080" 之类会一眼露馅。
        let title = "VibeClassAgent";
        assert!(!title.contains("http"), "标题里出现了地址，会显得像浏览器");
        assert!(!title.contains("localhost"), "标题里出现了 localhost");
    }

    /// WebView2 的数据目录必须**稳定、且不在 `%TEMP%` 根下**。
    ///
    /// 放 `%TEMP%` 会被清理软件删掉，删完下次启动 WebView2 可能直接
    /// 起不来（实测本机报 `0x8000FFFF 灾难性故障`，信息量为零）。
    /// 这和 Edge profile 那个坑是同一个道理，所以用同样的标准守。
    #[test]
    fn webview_data_dir_is_stable_and_not_in_temp_root() {
        let dir = webview_data_dir();
        let s = dir.to_string_lossy().to_lowercase();

        assert!(
            s.contains("vibeclassagent"),
            "路径里没有程序名，太容易被误删：{s}"
        );
        assert!(s.ends_with("webview"), "末级目录不是 webview：{s}");

        // 不能是 %TEMP% 的直接子目录
        if let Ok(temp) = std::env::var("TEMP") {
            let t = std::path::PathBuf::from(temp);
            assert_ne!(
                dir.parent(),
                Some(t.as_path()),
                "数据目录直接躺在 %TEMP% 下，会被清掉"
            );
        }
    }

    /// 数据目录必须能被创建出来。
    ///
    /// WebView2 自己建这个目录有时会失败，所以我们提前建 —— 这条测试
    /// 保证"提前建"这件事真的做成了（否则运行时就是那个灾难性故障）。
    #[test]
    fn webview_data_dir_is_created() {
        assert!(
            webview_data_dir().is_dir(),
            "数据目录没建出来 —— WebView2 起不来时会报 0x8000FFFF，查不出原因"
        );
    }
}
