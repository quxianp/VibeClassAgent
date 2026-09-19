# 插件开发指南

## 1. 插件包结构

```
my-plugin/
 manifest.toml      清单（必需）
 README.md          说明（建议）
 bin/               可执行体（process 模式必需）
 checksums.json     完整性校验（必需）
 signature.sig      作者签名（可选）
```

## 2. manifest.toml

```toml
[plugin]
id = "pusher.example"
name = "示例推送插件"
version = "1.0.0"
api_version = "1"              # 宿主接口大版本，不匹配则拒绝加载
kind = "pusher"                # recorder|transcriber|extractor|linker|doc|pusher|cleaner
author = "someone"
license = "MIT"
risk_note = "使用第三方协议存在账号风险，请自行评估"   # 可选；有值则安装时强制风险确认

[runtime]
type = "process"               # process（默认）| cdylib | wasm
entry = "bin/pusher-example.exe"
timeout_sec = 30
restart = "on-failure"

[permissions]                  # 安装时向用户展示并要求确认
network = ["https://example.com"]
filesystem = ["read:docs/**", "write:logs/**"]
env = ["EXAMPLE_TOKEN"]

[config]                       # JSON Schema，CLI 据此生成配置项并校验
token = { type = "string", secret = true, required = true }
target = { type = "string", required = true }
```

## 3. 通信协议

- 传输：**stdin / stdout**，每行一个 JSON 对象（NDJSON）；
- 协议：**JSON-RPC 2.0**；
- 类型定义见 `crates/vca-ipc/src/lib.rs`。

### 方法

| 方法 | 说明 | params | result |
|---|---|---|---|
| `describe` | 自述能力与 Schema | - | `DescribeResult` |
| `init` | 初始化并注入配置 | 配置对象 | `{ ok: bool }` |
| `run` | 执行主逻辑 | 按插件类型的载荷 | 按插件类型的载荷 |
| `cleanup` | 释放资源 | - | `{ ok: bool }` |
| `health_check` | 健康检查 | - | `HealthStatus` |

### 请求 / 响应示例

```json
{"jsonrpc":"2.0","id":1,"method":"run","params":{"videoPath":"...","audioPath":"..."}}
{"jsonrpc":"2.0","id":1,"result":{"segments":[{"start":0.0,"end":3.2,"text":"..."}]}}
{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"转写失败"}}
```

### 各类型载荷

| 类型 | `run` 输入 | `run` 输出 |
|---|---|---|
| recorder | `{slot, outputDir, params}` | `RecordResult` |
| transcriber | `{audioPath, language, model}` | `TranscriptResult` |
| extractor | `{segments, template}` | `{lessons: LessonSummary[]}` |
| linker | `{lessons, screenshots}` | `{linked: [...]}` |
| doc | `{lessons, screenshots, formats}` | `DocResult` |
| pusher | `{docPath, target, message}` | `PushResult` |
| cleaner | `{paths, policy}` | `CleanResult` |

## 4. 用 Python 写一个插件

```python
import json, sys

def handle(req):
    method = req.get("method")
    if method == "describe":
        return {"id": "pusher.example", "kind": "pusher", "version": "1.0.0",
                "api_version": "1", "capabilities": ["send"]}
    if method == "run":
        return {"success": True, "messageId": "msg-1"}
    return {}

for line in sys.stdin:
    if line.strip():
        req = json.loads(line)
        print(json.dumps({"jsonrpc": "2.0", "id": req.get("id"),
                          "result": handle(req)}, ensure_ascii=False), flush=True)
```

## 5. 安全要求（强制）

1. **凭据零硬编码**：token / webhook / password 只能来自环境变量或 `secrets.env`；
2. `manifest.toml` 中**不得**出现任何真实凭据；
3. 声明最小权限：只申请真正需要的 `network` / `filesystem` / `env`；
4. 涉及第三方协议的插件**必须**填写 `risk_note`；
5. 分发时提供 `checksums.json`。

## 6. 本地安装（v1）

```bash
vca plugin install ./path/to/my-plugin
vca plugin list
vca plugin doctor my-plugin
```

v1 **无在线市场**；`marketplace.remote_url` 配置项已预留但默认留空。
