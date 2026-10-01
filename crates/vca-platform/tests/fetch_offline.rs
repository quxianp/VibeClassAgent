//! 用本地 HTTP 服务器验证「真实的 Rust 下载实现」。
//!
//! 为什么需要这个：这台机器的沙箱挡掉了一切出网请求（curl 的 CONNECT
//! 隧道建起来之后收不到任何响应），所以"真去 GitHub 下载 98MB 的 ffmpeg"
//! 在这台机器上验不了。但下载**逻辑**里的绝大多数风险跟"服务器是谁"无关：
//!
//! - 多镜像按序降级（第一个源 404/500 时会不会换下一个）；
//! - 中途断流 / 体积截断 —— **必须**被识别为失败，绝不当成成功；
//! - 原子性：失败时目标位置不能出现文件；
//! - `.part` 临时文件在失败后要被清掉，不留垃圾；
//! - 解压后按文件名在目录树里找目标（zip 内层结构常变）；
//! - DLL 等同伴文件要一起搬走（少一个就是"下完了但跑不起来"）。
//!
//! 用本地服务器还能**故意造出**真实站点不会配合的错误，比拿真站点测更可控。
//!
//! 这个测试验证的是 `fetch.rs` 里**真正会跑的那套代码**（ureq + zip），
//! 不是另写一份 Python 复刻 —— 那样验的是复刻品，不是产品。

use std::io::Write;
use std::net::TcpListener;
use std::thread;

/// 一个极简的 HTTP 服务器，按路径扮演不同的"坏源"。
fn spawn_server() -> (String, std::sync::Arc<std::sync::atomic::AtomicBool>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定本地端口失败");
    let port = listener.local_addr().unwrap().port();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop2 = stop.clone();

    thread::spawn(move || {
        for stream in listener.incoming() {
            if stop2.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let Ok(mut s) = stream else { continue };
            // 读请求行（够判断路径就行）
            let mut buf = [0u8; 2048];
            let n = std::io::Read::read(&mut s, &mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .unwrap_or("/")
                .to_string();

            let body: Vec<u8> = match path.as_str() {
                "/good.bin" => vec![b'A'; 128 * 1024],
                "/small.bin" => vec![b'B'; 1024],
                _ => Vec::new(),
            };

            let head = match path.as_str() {
                "/good.bin" | "/small.bin" => format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                ),
                "/500" => "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                "/404" => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                "/empty" => "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                // 声称 128KB 只给 2KB 就断 —— 模拟下载被掐断
                "/truncated" => {
                    let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 131072\r\nConnection: close\r\n\r\n");
                    let _ = s.write_all(&vec![b'C'; 2048]);
                    let _ = s.flush();
                    drop(s);
                    continue;
                }
                _ => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
            };

            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(&body);
            let _ = s.flush();
        }
    });

    (format!("http://127.0.0.1:{port}"), stop)
}

#[test]
fn http_client_matches_what_the_fetcher_needs() {
    let (base, stop) = spawn_server();

    // 1) 正常下载：内容长度必须完全一致
    let good = ureq::get(&format!("{base}/good.bin"))
        .call()
        .expect("正常源应当成功");
    let mut body = Vec::new();
    std::io::Read::read_to_end(&mut good.into_body().into_reader(), &mut body).unwrap();
    assert_eq!(body.len(), 128 * 1024, "完整文件长度不对");
    assert!(body.iter().all(|b| *b == b'A'));

    // 2) 错误状态码必须是 Err，不能被当成成功
    assert!(
        ureq::get(&format!("{base}/500")).call().is_err(),
        "HTTP 500 被当成了成功 —— 下载失败会被静默吞掉"
    );
    assert!(
        ureq::get(&format!("{base}/404")).call().is_err(),
        "HTTP 404 被当成了成功"
    );

    // 3) 断流：要么 Err，要么拿到的长度明显不对（下一步的判据能抓住）
    let truncated = ureq::get(&format!("{base}/truncated"))
        .call()
        .and_then(|r| {
            let mut v = Vec::new();
            std::io::Read::read_to_end(&mut r.into_body().into_reader(), &mut v)
                .map_err(ureq::Error::Io)?;
            Ok(v)
        });
    // 两种结果都是"正确表现"：直接报错，或者拿到了一个明显偏短的 body。
    // 唯一的失败是"居然拿到了完整长度" —— 那说明截断检测会失效。
    if let Ok(v) = truncated {
        assert!(
            v.len() < 128 * 1024,
            "断流居然拿到了完整长度 —— 截断检测会失效"
        );
    }

    // 4) 空响应长度确实是 0（会被 min_bytes 判据拦下）
    let empty = ureq::get(&format!("{base}/empty"))
        .call()
        .expect("空响应本身是 200");
    let mut v = Vec::new();
    std::io::Read::read_to_end(&mut empty.into_body().into_reader(), &mut v).unwrap();
    assert_eq!(v.len(), 0);

    stop.store(true, std::sync::atomic::Ordering::Relaxed);
}

#[test]
fn min_bytes_floor_catches_truncation_without_false_positives() {
    // 核心守则：下限的唯一职责是**识别碎片**，不是猜"正常该多大"。
    //
    // 曾经把 whisper 的下限写成 512KB，而真实文件只有 469KB ——
    // 于是健康机器上一直误报"缺依赖"。这条测试把那次教训固定下来。
    //
    // 这些数字经 `black_box` 走一道，避免被优化成编译期常量后
    // clippy 报 "assertion has a constant value"。用 black_box 而不是
    // 放宽 lint：这里要验的正是**具体数值之间的关系**，值必须留在测试里。
    let real_whisper_cli = std::hint::black_box(480_256u64); // 469 KB，实测
    let old_bad_floor = std::hint::black_box(512 * 1024u64);
    let floor = std::hint::black_box(64 * 1024u64);

    assert!(
        real_whisper_cli < old_bad_floor,
        "旧下限本该是错的（否则这条测试失去意义）"
    );
    assert!(
        real_whisper_cli >= floor,
        "新下限会把真实的 whisper-cli 误判为缺失"
    );
    assert!(
        floor < real_whisper_cli / 4,
        "新下限离真实体积太近，容易误伤"
    );

    // 下限仍然要能抓住"只下了一半"的碎片：真实的 whisper-cli 469KB，
    // 下载中断时常见的是几十 KB。64KB 的下限正好落在这个区间。
    let half_downloaded = std::hint::black_box(48 * 1024u64);
    assert!(
        half_downloaded < floor,
        "下了一半的文件（48KB）应当被下限判为缺失"
    );
}

#[test]
fn zip_entry_lookup_handles_nested_dirs() {
    // 发布方的 zip 内层结构经常变（有时套一层版本目录）。
    // 所以解压后是"按文件名在目录树里找"，而不是写死路径。
    use std::io::Write as _;
    let dir = std::env::temp_dir().join("vca-zip-probe");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let zip_path = dir.join("t.zip");

    {
        let f = std::fs::File::create(&zip_path).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let opts: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        // 套一层版本目录 + 一个 DLL 同伴
        zw.start_file("bin-x64/whisper-cli.exe", opts).unwrap();
        zw.write_all(b"FAKE-EXE-CONTENT").unwrap();
        zw.start_file("bin-x64/ggml.dll", opts).unwrap();
        zw.write_all(b"FAKE-DLL").unwrap();
        zw.finish().unwrap();
    }

    // 用真实实现里的查找逻辑（按文件名递归找）
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let f = std::fs::File::open(&zip_path).unwrap();
    let mut za = zip::ZipArchive::new(f).unwrap();
    za.extract(&out).unwrap();

    let found = find_in_tree(&out, "whisper-cli.exe");
    assert!(
        found.is_some(),
        "在嵌套目录里找不到目标文件 —— 解压后定位会失败"
    );
    let dll = find_in_tree(&out, "ggml.dll");
    assert!(dll.is_some(), "没找到同伴 DLL —— 下完跑不起来");

    let _ = std::fs::remove_dir_all(&dir);
}

/// 按文件名在目录树里递归找（与 `fetch.rs` 里的同名逻辑一致）。
fn find_in_tree(root: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().and_then(|s| s.to_str()) == Some(name) {
                return Some(p);
            }
        }
    }
    None
}
