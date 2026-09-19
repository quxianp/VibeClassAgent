# plugins/  插件目录

本目录存放**内置插件**的占位骨架。插件以「目录 + manifest」形式分发，
运行时由 `vca-plugin-host` 扫描、校验并加载。

## 目录约定

```
<plugin-folder>/
  manifest.toml     插件清单：id / 版本 / 接口版本 / 类型 / 运行时 / 权限 / 配置 Schema
  README.md         职责说明与实现状态
  bin/              可执行体（待实现）
  checksums.json    完整性校验（待生成）
  signature.sig     作者签名（可选）
```

## 已规划的插件

| 目录 | 插件 id | 类型 | 职责 |
|---|---|---|---|
| `recorder/` | `recorder` | recorder | 静默录屏录音 |
| `transcriber/` | `transcriber` | transcriber | 语音转写（本地） |
| `extractor/` | `extractor` | extractor | 内容提取（LLM） |
| `screenshot-linker/` | `linker` | linker | 截图关联与去重 |
| `doc-generator/` | `doc` | doc | Word / PDF 生成 |
| `pusher-wecom/` | `pusher.wecom` | pusher | 企业微信机器人 |
| `pusher-qq/` | `pusher.qq` | pusher | 官方 QQ 机器人 |
| `pusher-wechat-personal/` | `pusher.wechat.personal` | pusher | 个人微信（第三方，带风险提示） |
| `cleaner/` | `cleaner` | cleaner | 72 小时到期清理 |

## 插件接口

所有插件必须实现 `vca-ipc` 中定义的方法：

```
init / run / cleanup / health_check / describe
```

通信方式：**JSON-RPC 2.0 over stdio**（默认），宿主与插件通过 stdin/stdout 交换消息。

## 运行模式

| 模式 | 说明 |
|---|---|
| `process`（默认） | 独立进程，隔离性最好，任何语言都可实现 |
| `cdylib` | 动态库直接加载，低延迟 |
| `wasm` | WASI 沙箱，最安全 |

## 本地市场（v1）

`vca market list` / `vca plugin install <本地路径>`。
v1 **不做在线市场**；`marketplace.remote_url` 配置项已预留但默认留空。

## 安全约束

- 插件凭据一律走环境变量或 `secrets.env`，**禁止**写入 `manifest.toml` 或入库；
- 安装时校验 `checksums.json` 与 `api_version`，并向用户展示权限与风险后确认。
