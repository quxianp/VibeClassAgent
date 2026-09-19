# 插件：recorder

**类型**：`recorder`
**插件 id**：`recorder`
**状态**：占位（当前录制由宿主内置实现承担，本插件尚未实现）

## 职责

静默录屏录音。宿主内置实现走 ffmpeg 录屏 + WASAPI 采集系统声音与麦克风。

## 实现状态

| 项 | 状态 |
|---|---|
| manifest.toml | 已就位 |
| README | 已就位 |
| 可执行体 `bin/vca-plugin-recorder.exe` | **待实现** |
| checksums.json | **待生成** |
| signature.sig | 可选，待定 |

> 想写这个插件的话，照抄 `plugins/example-echo/` ——
> 那是一个功能完整、能直接跑的最小实现（源码在 `crates/vca-plugin-example/`），
> 里面把协议交互、stderr 日志约定、`entry` 路径规则都演示了。

## 待办

1. 实现插件可执行体，遵循 `vca-ipc` 定义的 JSON-RPC 方法：
   `init` / `run` / `cleanup` / `health_check` / `describe`；
2. 生成 `checksums.json`；
3. 补齐 `[config]` 段的 JSON Schema（供 CLI 生成配置表单）；
4. 若涉及第三方协议或敏感权限，填写 `risk_note` 并在安装时向用户确认。

