# 开发与调试

## 1. 环境要求

| 依赖 | 版本 | 必需 |
|---|---|---|
| Rust | stable（1.75） | 是 |
| Python | 3.10 | 否（仅算法侧） |
| ffmpeg | 任意近期版本 | 否（仅实际录制） |
| LibreOffice | 任意近期版本 | 否（仅 PDF 导出） |

## 2. 构建

```bash
cargo build            # 调试构建
cargo build --release  # 发布构建（已开启 LTO + strip + opt-level=z）
cargo check            # 快速语法检查
```

## 3. 运行

```bash
cargo run -p vca-cli                       # 显示横幅
cargo run -p vca-cli -- doctor             # 环境自检
cargo run -p vca-cli -- config path        # 查看目录
cargo run -p vca-cli -- --profile 张三      # 指定 profile
```

## 4. 测试与静态检查

```bash
cargo test                                  # 单元测试
cargo fmt --all                             # 格式化
cargo clippy --all-targets -- -D warnings    # 静态检查
```

## 5. 日志

通过 `RUST_LOG` 控制级别：

```bash
RUST_LOG=debug cargo run -p vca-cli -- doctor
```

Windows PowerShell：

```powershell
$env:RUST_LOG="debug"; cargo run -p vca-cli -- doctor
```

## 6. 目录覆盖（便于开发调试）

```bash
cargo run -p vca-cli -- --data-dir ./data --config-dir ./config config path
```

## 7. 本机工具链特殊情况

若本机无 MSVC 链接器（如仅有 Build Tools 但缺 C++ 工作负载），使用 GNU 目标：

```powershell
$env:CARGO_HOME  = "D:\VibeClassAgent\.toolchain\cargo"
$env:RUSTUP_HOME = "D:\VibeClassAgent\.toolchain\rustup"
cargo build
```

或安装 MSVC C++ 工作负载后再用默认目标。

## 8. 新增一个插件（SOP）

1. 在 `plugins/` 下新建目录，写入 `manifest.toml`；
2. 实现可执行体，遵循 `docs/PLUGIN_SDK.md` 的协议；
3. 生成 `checksums.json`；
4. 补全 `[config]` Schema；
5. 若涉及第三方协议，填写 `risk_note`。

## 9. 新增一个 crate

1. 在 `crates/` 下创建目录与 `Cargo.toml`，字段用 `workspace = true`；
2. 在根 `Cargo.toml` 的 `members` 中登记；
3. 如需共享依赖，在 `[workspace.dependencies]` 声明后引用。

## 10. 代码规范

- `#![forbid(unsafe_code)]`：核心与 CLI 禁止 unsafe；
- `#![warn(missing_docs)]`：公开项必须有文档注释；
- 中文注释用于说明「为什么」，代码本身表达「是什么」；
- 所有未实现的函数返回明确错误或 `TODO` 注释附策划书出处，**不要留空实现**。
