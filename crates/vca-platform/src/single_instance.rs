//! 单实例保护。
//!
//! # 为什么需要它
//!
//! 界面进程是「开着一个后台服务 + 一个托盘图标 + 一个浏览器窗口」的组合。
//! 没有单实例保护时，用户**多双击几次就会出现多套**：
//!
//! - 每个实例都往任务栏塞一个托盘图标（用户以为只有一个程序在跑）
//! - 每个实例都拉起一个 Edge 窗口
//! - 每个实例都常驻不退出，内存线性叠加
//!
//! 这不是假想的场景：当界面迟迟不出来时（比如浏览器起不来），用户的本能
//! 反应就是**再双击一次**。每点一次就多一个常住进程，机器很快就卡了 ——
//! 用户看到的是「程序占用很高、容易崩」，而根因其实是没人拦着重复启动。
//!
//! # 为什么用命名互斥体而不是锁文件
//!
//! 锁文件要靠**进程退出时主动删除**。程序被强杀（任务管理器结束进程、
//! 蓝屏、断电）就留下一个残留文件，下次启动会误判「已有实例在跑」而拒绝启动 ——
//! 而且用户没有任何办法自己修（他不知道该删哪个文件）。
//!
//! 命名互斥体由**内核**持有：进程一死（无论怎么死）句柄自动释放，
//! 不存在残留。这是 Win32 单实例的标准做法。

/// 已经持有的互斥体句柄。
///
/// 故意做成「拿到就一直不释放、直到进程结束」：互斥体的所有权必须覆盖
/// 整个程序生命周期，提前 drop 会让下一个实例误以为没人跑。
static INSTANCE_MUTEX: std::sync::OnceLock<isize> = std::sync::OnceLock::new();

/// 尝试成为唯一实例。
///
/// - 返回 `true`：本进程是唯一实例（或本进程此前已获取过，幂等）。
/// - 返回 `false`：**已经有另一个实例在运行**，调用方应当把用户引到已有窗口
///   然后退出，而不是继续启动第二个后台服务。
///
/// 非 Windows 平台永远返回 `true`（本项目只发布 Windows，这里不制造额外分支）。
#[cfg(windows)]
pub fn acquire(name: &str) -> bool {
    // 已经拿到过：直接成功。避免第二次调用再去 CreateMutex 而误判。
    if INSTANCE_MUTEX.get().is_some() {
        return true;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateMutexW(
            attrs: *const core::ffi::c_void,
            initial_owner: i32,
            name: *const u16,
        ) -> isize;
        fn GetLastError() -> u32;
        fn CloseHandle(h: isize) -> i32;
    }

    /// `ERROR_ALREADY_EXISTS`：同名互斥体已存在 = 已有实例在跑。
    const ERROR_ALREADY_EXISTS: u32 = 183;

    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();

    // SAFETY: 传入的是本函数内构造、生命周期覆盖整个调用的宽字符串；
    // 返回句柄由我们保管（存入静态变量），并在冲突时立即关闭。
    let handle = unsafe {
        // 第二个参数传 0：不要求「立刻占有」，只要它存在就能判重。
        // 传 1 的话会变成一把需要配对的锁，而我们要的只是一个「存在性标记」。
        let h = CreateMutexW(core::ptr::null(), 0, wide.as_ptr());
        if h == 0 {
            // 创建都失败（极端情况）时**放行**：宁可多开一个，
            // 也不要因为拿不到互斥体而让用户完全打不开程序。
            return true;
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(h);
            return false;
        }
        h
    };

    // 把句柄存进静态变量，让它活到进程结束（互斥体的所有权必须覆盖整个生命周期）。
    // `set` 返回 Err 表示已有别的线程抢先设置过 —— 那说明并发调用，
    // 我们手里这个多出来的句柄没人会用，关掉它避免泄漏。
    match INSTANCE_MUTEX.set(handle) {
        Ok(()) => true,
        Err(_) => {
            // SAFETY: handle 是本函数刚创建的有效句柄，没有被别处持有。
            unsafe {
                CloseHandle(handle);
            }
            true
        }
    }
}

/// 非 Windows：不做限制。
#[cfg(not(windows))]
pub fn acquire(_name: &str) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_acquire_succeeds_and_is_idempotent() {
        // 用一条独特的名字，避免和其它测试/真实实例撞车
        let name = "Global\\VCA_SingleInstance_test_idempotent";
        assert!(acquire(name), "首次获取应当成功");
        // 同进程重复获取必须还是 true —— 否则程序自己会把自己挡住
        assert!(acquire(name), "同一进程重复获取应当仍然成功");
    }

    #[test]
    fn acquiring_does_not_panic_for_odd_names() {
        let _ = acquire("Global\\VCA_test_odd_name_中文_白名单");
    }
}
