# 后端 → 前端互通说明：用户自填下载镜像源

> 状态：后端实现与验证完成；等待前端指定模型路由可用后接入 UI。
> 默认策略已由用户确认：未填写时保留现有公开源列表，不新增单一强制镜像。

## 1. 后端改动文件

| 文件 | 改动摘要 |
|---|---|
| `crates/vca-core/src/config.rs` | 新增 `DownloadSettings`；实现 URL/前缀校验、前缀规范化、自定义源与默认源的合并顺序。 |
| `crates/vca-platform/src/fetch.rs` | 新增 `fetch_missing_with_mirrors`，把用户配置传入真实下载循环；暴露只读默认源清单；增加真实本地 HTTP 透传测试。 |
| `crates/vca-platform/src/http.rs` | 新增镜像轻量探测：先 HEAD，不支持则 Range GET；识别 200 HTML 登录页；读取 `Content-Length` / `Content-Range`。 |
| `crates/vca-gui/src/api.rs` | 新增镜像读取、覆盖式保存与测试 API；持久化到当前 profile 的 `settings.yaml`；下载开始时重新读取配置。 |
| `locales/zh-CN.json` | 新增 `web.deps.mirror_*` 中文 UI 文案。 |
| `locales/en-US.json` | 同步新增英文 UI 文案。 |
| `web/CONTRACT.md` | 写明三个接口、字段、错误、默认源策略及即时生效条件。 |

## 2. 配置入口与持久化

采用“**前端表单 + profile 配置文件**”组合；没有新增环境变量。

配置文件位置：

```text
<config_root>/profiles/<profile>/settings.yaml
```

YAML 数据结构：

```yaml
download:
  prefix:
    - https://mirror.example/
  mirrors:
    ffmpeg:
      - https://files.example/ffmpeg.zip
    whisper:
      - https://files.example/whisper.zip
    model:
      - https://files.example/ggml-tiny.bin
  only_custom: false
```

字段含义：

- `prefix: string[]`：全局加速前缀。后端会把它拼到每个默认 URL 前；带或不带结尾 `/` 均可，保存时规范为恰好一个 `/`。
- `mirrors: object<string,string[]>`：每个依赖的**完整文件 URL**。允许的键仅为 `ffmpeg`、`whisper`、`model`；一行一个、按顺序尝试。
- `only_custom: boolean`：`true` 时不再尝试默认公开源；至少要有一个自定义完整 URL 或前缀。

镜像 URL 不属于凭据，不会写入 secrets；但后端明确拒绝 `user:password@host` 形式，避免凭据进入日志和配置仓库。

## 3. 默认下载源策略

用户未填写时保持原有公开源顺序：

- ffmpeg：`gyan.dev` → GitHub；
- whisper.cpp：GitHub → `ghfast.top`；
- tiny 模型：`hf-mirror.com` → HuggingFace。

用户配置存在时，真实下载顺序固定为：

1. 该依赖的完整自定义 URL；
2. 全局前缀 + 默认 URL；
3. 默认公开 URL（`only_custom=false` 时）。

后端不会用硬编码值覆盖用户输入。真实下载器测试会启动本地 HTTP 服务，并故意把默认源设为不可用地址，证明请求首先到达用户传入 URL。

## 4. API 契约

### 4.1 读取

```http
GET /api/deps/mirrors
```

示例：

```json
{
  "ok": true,
  "prefix": ["https://mirror.example/"],
  "mirrors": {
    "ffmpeg": ["https://files.example/ffmpeg.zip"]
  },
  "only_custom": false,
  "builtin": {
    "ffmpeg": ["https://www.gyan.dev/...", "https://github.com/..."],
    "whisper": ["https://github.com/...", "https://ghfast.top/..."],
    "model": ["https://hf-mirror.com/...", "https://huggingface.co/..."]
  },
  "deps": [
    {
      "id": "ffmpeg",
      "label": "ffmpeg（录屏与音视频合并）",
      "required": true,
      "consequence": "无法录制屏幕，程序完全不能工作"
    }
  ]
}
```

前端应按 `deps` 动态渲染，不要自己硬编码依赖清单；`builtin` 只读展示。

### 4.2 保存（整体覆盖）

```http
POST /api/deps/mirrors
Content-Type: application/json
```

```json
{
  "prefix": ["https://mirror.example"],
  "mirrors": {
    "ffmpeg": ["https://files.example/ffmpeg.zip"]
  },
  "only_custom": false
}
```

成功时返回回读磁盘后的规范化值，例如前缀会变成 `https://mirror.example/`。

失败仍是 HTTP 200，以 `ok:false` 判断；`field` 指向应标红的字段：

```json
{
  "ok": false,
  "error": "ffmpeg 的地址……不合法：只支持 http:// 或 https:// 协议",
  "field": "ffmpeg"
}
```

可返回的 `field`：`body`、`prefix`、`mirrors`、`ffmpeg`、`whisper`、`model`、`only_custom`。

### 4.3 测试单个完整 URL

```http
POST /api/deps/mirrors/test
Content-Type: application/json

{"url":"https://files.example/ffmpeg.zip"}
```

成功：

```json
{
  "ok": true,
  "status": 200,
  "kind": "文件",
  "size": 102906624,
  "message": "连通正常（HTTP 200，判定为文件，约 98 MB）"
}
```

失败：

```json
{
  "ok": false,
  "error": "连不上：……",
  "field": "url",
  "hint": "确认地址能在浏览器里直接下载文件……"
}
```

探测最多阻塞 8 秒。它不会下载整个文件：HEAD 不支持时只请求 `Range: bytes=0-0`，并且不读取响应 body。若服务器返回 `200 text/html`（如校园网登录页），后端会判定为无效文件 URL。

## 5. 边界规则

- 空 `prefix`、空 `mirrors`、`only_custom=false`：合法，表示使用默认公开源。
- 非 `http(s)`、缺协议、缺主机、非法端口、空格、反斜杠：拒绝。
- URL 中含 `user:password@host`：拒绝，防止凭据落盘或进入日志。
- 前缀含 `?` 或 `#`：拒绝；应改填每个依赖的完整 URL。
- 前缀有无结尾斜杠：均接受并规范化。
- 未知依赖 id、字段类型错误、数组元素非字符串：拒绝，返回具体 `field`。
- `only_custom=true` 且所有自定义字段为空：拒绝。
- 用户手工把非法 URL 写进 YAML：真实下载合并时再次过滤，不让非法值进入网络层。
- 现有 `settings.yaml` 损坏：拒绝保存，不覆盖用户其它设置。

## 6. 生效条件

保存后**无需重新编译，也无需重启服务**。

`POST /api/deps/fetch` 每次开始下载时重新读取当前 profile 的配置，因此下一次下载立即生效。已经启动的下载持有启动瞬间的配置快照，不会在下载中途切源。

## 7. 前端需要配合

1. 在依赖下载面板加入镜像设置入口；使用现有 `deps.mirror_*` i18n 键，不硬编码中文。
2. `mirrors` 使用完整 URL 多行输入；`prefix` 单独放在高级设置，明确它不是完整文件地址。
3. 保存成功后用响应里的规范化值覆盖本地表单状态。
4. 错误时按 `field` 标红；不能只看 HTTP 状态码。
5. 地址测试时给出最多 8 秒的进行中状态；不要增加常驻轮询。
6. 重置应 POST 空配置，而不是只清空 DOM。
7. 展示 `builtin` 默认源，但不可编辑。

## 8. 自检结果

- 后端聚焦测试：
  - `vca-core` 镜像配置/合并：11/11；
  - `vca-platform` 全部：221 通过、1 ignored；
  - `vca-gui` 镜像 API：4/4。
- `scripts\gate.ps1`：
  - `cargo fmt --all -- --check`：通过；
  - `cargo clippy --workspace --all-targets -- -D warnings`：通过，0 告警；
  - `cargo test --workspace`：422 通过、0 失败、2 ignored（需要外网/文档示例）。
- `python scripts\smoke.py`：**58/58 全部通过**。
- 真实 HTTP API 往返探针（当前 debug exe）：
  - 空配置回出 3 个依赖、每个 2 个默认公开源；
  - `https://mirror.example///` 保存后规范为 `https://mirror.example/`；
  - 同一进程立即 GET 到新值，确认无需重启；
  - 非 http(s)、带查询串前缀、未知依赖 id、空 `only_custom` 共 4 类非法输入全部拒绝；
  - 重置后恢复默认源策略。
- 凭据扫描：diff 未发现 token/私钥/password；Git remote URL 无内嵌凭据；
  `.env`、`secrets.env`、`*.pem`、`*.key`、`*credentials*`、`*_rsa` 均被 `.gitignore` 覆盖。

## 9. 已知问题与待确认

- 真实公网大文件下载仍受当前网络环境影响；本轮通过本地 HTTP 服务器验证真实 Rust 下载链路、候选顺序、Range 探测与落盘，不伪称已完成公网 98 MB 下载。
- 第三方镜像可能随时失效；这是开放用户配置入口的根本原因。默认列表只是回退，不承诺永久可用。
- 前端由指定的 `xindu/kimi-k2.7-code` 模型负责；当前模型路由需在团队供应商中配置后才能接续运行。
