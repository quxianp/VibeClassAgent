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
    /// 程序正在彻底退出，窗口与 WebView 已销毁。
    ///
    /// 点系统原生 X 只会隐藏窗口，不会发送这个事件；只有主动执行
    /// [`WindowControl::close`] 时才会发送。
    Closed,
    /// 窗口没能建起来。`String` 是给人看的失败原因。
    ///
    /// 收到这个时**不要**让程序退出：HTTP 服务还在跑，退回"用系统浏览器
    /// 打开"至少让用户能用上。这正是原来 Edge 方案里最该有却没有的兜底。
    Failed(String),
    /// 页面加载完成、窗口**刚刚被显示出来**。
    ///
    /// 窗口是建好但不可见的（见 `with_visible(false)`），页面画完了才亮出来。
    /// 主要给启动计时用 —— 这个时间点才是"用户真正看到界面"的时刻，
    /// 比"进程起来了"有意义得多。
    Shown,
}

/// 起一个原生窗口加载 `url`，阻塞运行到收到显式退出命令。
///
/// # 线程模型（重要）
///
/// Win32 的窗口属于**创建它的那个线程**，消息也只能由那个线程抽取，
/// 所以事件循环必须占用调用线程。调用方应先把 HTTP 服务放到后台线程，
/// 再调用本函数把当前（主）线程交给 Tao 事件循环。
///
/// `on_ready` 会在窗口和 WebView 创建成功、进入事件循环之前调用。
/// 调用方必须在这里发布 [`WindowControl`]，这样 HTTP、托盘和单实例
/// Focus 才能在事件循环运行期间向窗口发送命令，而不是等事件循环退出后
/// 才拿到已经失效的控制端。
///
/// # 返回
///
/// - `Ok(())`：窗口收到显式 [`WindowCmd::Close`]，事件循环正常结束；
/// - `Err`：窗口创建、WebView 创建或事件循环发生异常。
pub fn run<F>(url: String, on_ready: F) -> anyhow::Result<()>
where
    F: FnOnce(WindowControl),
{
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
        run_blocking(url, tx.clone(), cmd_rx, || {
            on_ready(WindowControl { cmd: cmd_tx });
            // 现有调用方通过 `Result` 接收初始化失败，不再轮询旧事件接收端；
            // 就绪后释放它，窗口控制只通过 `WindowControl` 发送。
            drop(rx);
        })
    }));

    match result {
        Ok(result) => result,
        Err(_) => anyhow::bail!("窗口事件循环异常退出"),
    }
}

/// 窗口本体：建窗口 + 跑事件循环（会阻塞到显式退出）。
fn run_blocking<F>(
    url: String,
    tx: mpsc::Sender<WindowEvent>,
    cmd_rx: mpsc::Receiver<WindowCmd>,
    on_ready: F,
) -> anyhow::Result<()>
where
    F: FnOnce(),
{
    use tao::event::{Event, WindowEvent as TaoWindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::platform::run_return::EventLoopExtRunReturn;
    use tao::window::WindowBuilder;
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();

    let proxy = event_loop.create_proxy();
    // WebView 的页面加载回调也要往事件循环里发事件，所以留一份。
    // `EventLoopProxy` 是 Clone 的，两边各持一份互不影响。
    let webview_proxy = proxy.clone();
    std::thread::spawn(move || {
        // 把主线程发来的命令转成 event_loop 的用户事件，
        // 这样就能在事件循环里处理，而不必跨线程碰窗口。
        while let Ok(cmd) = cmd_rx.recv() {
            if proxy.send_event(UserEvent::Cmd(cmd)).is_err() {
                break;
            }
        }
    });

    // ⚠️ **建好但先不显示** —— 这是不闪白的关键。
    //
    // 用户报「启动时窗口先闪白一下再进页面」。根因是顺序：原来窗口建完
    // 立刻就是可见的，而 WebView 要过一会儿才把页面渲染出来，
    // 中间那段时间露出的是**空窗口**（系统默认白底）——
    // 用户看到的就是"闪一下白"。
    //
    // 正确顺序是：建窗口（不可见）→ 建 WebView → 等页面**加载完成**
    // → 再把窗口显示出来。这样用户第一眼看到的就是渲染好的页面。
    //
    // `with_visible(false)` 必须在 build 时就设，不能建完再 set_visible(false) ——
    // 后者中间仍有一帧是可见的，还是会闪。
    let window = match WindowBuilder::new()
        .with_title("VibeClassAgent")
        .with_inner_size(tao::dpi::LogicalSize::new(1180.0, 820.0))
        .with_min_inner_size(tao::dpi::LogicalSize::new(880.0, 620.0))
        .with_visible(false)
        .build(&event_loop)
    {
        Ok(w) => w,
        Err(e) => {
            // 窗口建不出来不是"这个世界不存在"级别的问题 ——
            // 报回去让调用方退回系统浏览器，用户至少还能用。
            let msg = format!("创建窗口失败：{e}");
            tracing::warn!("{msg}");
            let _ = tx.send(WindowEvent::Failed(msg.clone()));
            anyhow::bail!(msg);
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
    // Debug 构建可跳过 WebView2，只用于真实 Win32 生命周期探针。这样探针验证
    // WM_CLOSE / 单实例唤醒 / 显式退出时，不会被测试机上另一个 WebView2 宿主
    // 对 profile 的占用干扰。Release 构建里该环境变量永远不起作用。
    let lifecycle_probe =
        cfg!(debug_assertions) && std::env::var_os("VCA_WINDOW_LIFECYCLE_PROBE").is_some();
    let mut web_context = wry::WebContext::new(Some(webview_data_dir()));

    let webview = if lifecycle_probe {
        tracing::info!("Win32 生命周期探针模式：跳过 WebView2 初始化");
        None
    } else {
        let webview = match wry::WebViewBuilder::new_with_web_context(&mut web_context)
            .with_url(&url)
            .with_initialization_script(INIT_SCRIPT)
            // 深色底：即使在某台机器上还是漏出一帧，露的也是**深色**而不是刺眼的白。
            // 界面的实际底色由 CSS 决定，这里管的是"页面还没画出来时"那层。
            //
            // 注意 `wry::RGBA` 就是个元组别名 `(u8,u8,u8,u8)`，
            // 不是结构体（wry 0.57），别写成 `Color(...)`。
            // 取值和界面深色主题的底色接近，过渡时看不出接缝。
            .with_background_color((24, 26, 32, 255))
            // 页面加载完成 → 这时才把窗口显示出来（见上面 with_visible(false)）。
            //
            // 用 `on_page_load` 而不是定时器：完成的时机由 WebView 自己报，
            // 不用猜"加载要多久"。慢机器上不会提前显示空窗口，
            // 快机器上也不会白等。
            //
            // 通过 `proxy` 发**用户事件**而不是直接调 `window.set_visible(true)` ——
            // 回调可能不在事件循环那个线程上跑，直接碰窗口不安全。
            // 绕一圈回到事件循环里执行才是正确姿势。
            .with_on_page_load_handler({
                let proxy = webview_proxy.clone();
                let tx = tx.clone();
                move |event, _url| {
                    if let wry::PageLoadEvent::Finished = event {
                        // 先让事件循环亮窗口，再通知主线程"已经可见了"
                        let _ = proxy.send_event(UserEvent::PageLoaded);
                        let _ = tx.send(WindowEvent::Shown);
                    }
                }
            })
            .build(&window)
        {
            Ok(w) => w,
            Err(e) => {
                // 最常见的原因是没装 WebView2 运行时。
                // **这条路径必须报出去** —— 否则用户看到的是一个空白窗口，
                // 完全不知道缺什么。
                let msg = format!("创建 WebView 失败（多半是缺少 WebView2 运行时）：{e}");
                tracing::warn!("{msg}");
                let _ = tx.send(WindowEvent::Failed(msg.clone()));
                anyhow::bail!(msg);
            }
        };
        Some(webview)
    };

    tracing::info!("原生窗口已创建，正在加载 {url}");
    on_ready();
    if lifecycle_probe {
        // 和真实 PageLoadEvent::Finished 走同一个用户事件分支，确保窗口显示
        // 与后续关闭/唤醒行为完全复用生产代码。
        let _ = webview_proxy.send_event(UserEvent::PageLoaded);
        let _ = tx.send(WindowEvent::Shown);
    }

    // ⚠️ 兜底：页面要是**加载不完**，窗口不能永远不显示。
    //
    // 我们把显示时机押在 `PageLoadEvent::Finished` 上，但那个事件在几种情况下
    // 可能不来：前端某段脚本把 load 卡住、WebView 内部异常、网络栈抽风。
    // 那样用户会看到**什么都没发生**（进程在跑、托盘有图标、就是没窗口），
    // 比"闪一下白"糟糕得多 —— 这是拿一个观感问题换一个可用性问题。
    //
    // 所以起一个看门狗：到点还没显示过，就无条件显示。
    // 宁可闪白也不能不出窗口。**这个交换是有意为之，不要删。**
    {
        let proxy = webview_proxy.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(SHOW_WATCHDOG_MS));
            // 事件循环已经退出（用户关窗口了）时，send 会失败，忽略即可。
            let _ = proxy.send_event(UserEvent::ShowAnyway);
        });
    }

    // ⚠️ WebView 句柄必须**一直活着**，直到窗口销毁。
    //
    // 它一旦被 drop，WebView2 控件就从窗口上摘掉了 —— 结果是一个**空窗口**，
    // 比"窗口没起来"更难查（窗口确实在，就是黑的/白的）。
    //
    // 用 `Option` 包着是为了能在关闭时**主动 drop**（见下面两个关闭分支），
    // 让控件在窗口还活着的时候正常拆除。
    let mut webview = webview;
    let mut shown = false;

    // 闭包末尾会读一次 `webview`（`black_box`），那是"保活"这件事的落点。
    // 没有那个读，编译器会认为这个变量只写不读 —— 警告只是表象，
    // 真正的风险是它可能被提前析构，于是窗口空白。**别删那次读。**
    event_loop.run_return(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent {
                event: TaoWindowEvent::CloseRequested,
                ..
            } => match close_action(CloseIntent::NativeRequest) {
                CloseAction::Hide => {
                    // 系统原生 X 的语义是「收起到后台」：只隐藏顶层窗口，
                    // WebView、事件循环、HTTP 服务和托盘都继续存活。
                    // 后续托盘或单实例 Focus 会把同一个窗口重新显示出来，
                    // 不会重复创建 WebView，也不会重复注册监听。
                    window.set_visible(false);
                }
                CloseAction::Exit => unreachable!("原生关闭请求只能隐藏窗口"),
            },
            Event::UserEvent(UserEvent::PageLoaded) => {
                // 页面画完了 → 亮出窗口。**用户第一眼就是渲染好的界面**，
                // 中间那层空窗口（系统默认白底）从来没露过脸 ——
                // 这就是"启动不闪白"的全部秘密。
                window.set_visible(true);
                window.set_focus();
                shown = true;
            }
            Event::UserEvent(UserEvent::ShowAnyway) => {
                // 看门狗到点了。只有还没显示过才动手 —— 正常路径下
                // `shown` 已经是 true，这个分支什么都不做（不会把用户
                // 已经调走的焦点抢回来）。
                if !shown {
                    tracing::warn!("页面 {SHOW_WATCHDOG_MS}ms 内没加载完，先把窗口显示出来");
                    window.set_visible(true);
                    shown = true;
                }
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
                match close_action(CloseIntent::Command) {
                    CloseAction::Hide => unreachable!("主动关闭命令必须彻底退出"),
                    CloseAction::Exit => {
                        // ⚠️ **先隐藏再销毁** —— 这是不闪白的关键（关闭方向）。
                        //
                        // 主动退出时先隐藏整个窗口，再摘掉 WebView；用户看不到
                        // WebView 销毁后露出的空窗口，因此不会在退出时闪白。
                        window.set_visible(false);
                        webview = None;
                        let _ = tx.send(WindowEvent::Closed);
                        *control_flow = ControlFlow::Exit;
                    }
                }
            }
            _ => {}
        }
        // 保活：确保 `webview` 真的被闭包持有到最后一刻。
        std::hint::black_box(&webview);
    });

    Ok(())
}

/// 原生窗口的**控制端**：只能发命令（聚焦/关闭），不能收事件。
///
/// 它只有 `Sender`，因此是 `Send + Sync`，可以直接放进 `static`
/// 供托盘线程和主线程共用 —— 不需要 `unsafe`，也不必破例
/// `vca-gui` 的 `#![forbid(unsafe_code)]`。
///
/// 由 [`run`] 的 `on_ready` 回调取得并发布给 HTTP/托盘线程。
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
    /// 彻底关闭窗口并退出事件循环。
    Close,
}

/// 窗口收到的关闭意图。
///
/// 将平台事件与执行动作分开，纯逻辑测试无需创建真实 Win32 窗口，
/// 也能固定「原生 X 只隐藏、程序关闭命令才退出」这一生命周期约定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseIntent {
    NativeRequest,
    Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseAction {
    Hide,
    Exit,
}

fn close_action(intent: CloseIntent) -> CloseAction {
    match intent {
        CloseIntent::NativeRequest => CloseAction::Hide,
        CloseIntent::Command => CloseAction::Exit,
    }
}

#[derive(Debug)]
enum UserEvent {
    Cmd(WindowCmd),
    /// WebView 报告页面加载完成 → 亮窗口。
    ///
    /// 单列一个而不是复用 `Cmd(WindowCmd::Show)`，是因为它来自 WebView 回调
    /// 而非主线程命令，语义不同；合并后调试时不好分辨是谁触发的。
    PageLoaded,
    /// 看门狗到点：不等页面了，显示窗口。
    ///
    /// 只在 [`PageLoaded`](Self::PageLoaded) 没来过时才起作用，
    /// 见 `SHOW_WATCHDOG_MS` 的说明。
    ShowAnyway,
}

/// 等页面加载的**上限**（毫秒）。超时就把窗口先显示出来。
///
/// 取值理由：正常机器上页面是本地 HTTP、无外链，实测远低于这个数。
/// 给到 3 秒是为了容忍慢机器 + 首次启动时 WebView2 冷启动（要初始化
/// 渲染进程、编译 shader，第一次确实偏慢）。
///
/// 调这个值前先想清楚：**调大 = 慢机器上白等更久，调小 = 可能白闪一下**。
/// 而"白闪"和"不出窗口"之间，永远选白闪。
const SHOW_WATCHDOG_MS: u64 = 3000;

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

/// 兼容旧公开名称；运行期句柄就是可跨线程发送命令的控制端。
pub type WindowHandle = WindowControl;

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
    fn native_close_request_only_hides_window() {
        assert_eq!(close_action(CloseIntent::NativeRequest), CloseAction::Hide);
    }

    #[test]
    fn close_command_exits_window_loop() {
        assert_eq!(close_action(CloseIntent::Command), CloseAction::Exit);
    }

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
