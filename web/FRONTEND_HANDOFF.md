# 前端 → 后端互通说明：下载镜像源设置

> 状态：镜像设置 UI 已接入 `web/`，最终测试结果见第 5 节。
> 边界：本文件只描述浏览器/WebView2 前端；接口语义以 `web/CONTRACT.md` 为准。

## 1. 改动文件

| 文件 | 改动 |
|---|---|
| `web/app.js` | 在“录制与文件”页读取并渲染镜像设置；支持依赖专属完整 URL、全局前缀、默认源展示、`only_custom`、单 URL 测试、保存与重置。 |
| `web/style.css` | 新增紧凑镜像卡片、URL 行、默认源列表、状态反馈和窄屏布局；没有新增框架、字体或运行时依赖。 |
| `web/FRONTEND_HANDOFF.md` | 本互通说明。 |

入口仍是 `web/index.html`，没有增加构建步骤，浏览器/WebView2 直接加载现有静态文件。

## 2. 接口与数据结构

### 2.1 首次读取

进入“录制与文件”页时并行请求：

```http
GET /api/config/general?t=<token>
GET /api/deps/mirrors?t=<token>
```

镜像响应示例：

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
    {"id": "ffmpeg", "label": "ffmpeg…", "required": true}
  ]
}
```

前端按后端返回的 `deps` 顺序渲染；若 `deps` 暂时缺失，会从 `builtin` 和 `mirrors` 键生成回退列表，不在前端硬编码依赖名称。

### 2.2 保存

```http
POST /api/deps/mirrors?t=<token>
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

- `mirrors` 输入是**完整文件 URL**，每行一个、保持顺序；空行不发送。
- `prefix` 是**全局加速前缀**，单独置于高级设置；每行一个。
- 保存成功后使用后端响应中的规范化值重新渲染，因此结尾斜杠等展示与实际落盘一致。
- `only_custom=true` 时，只要完整 URL 或全局前缀任一种非空即可保存；两者都为空时前端先拦截，后端仍有最终校验。
- 重置会真实 POST `{prefix:[], mirrors:{}, only_custom:false}`，不是只清 DOM。
- 保存对下一次下载生效；正在进行的下载不被中途切源。

### 2.3 测试单个完整 URL

```http
POST /api/deps/mirrors/test?t=<token>
Content-Type: application/json

{"url":"https://files.example/ffmpeg.zip"}
```

每个完整 URL 输入行旁有“测试”按钮。前端使用 `AbortController` 设置 8 秒上限，并显示后端返回的 `message` 或 `error`。全局前缀不是完整文件 URL，不提供单独测试按钮。

## 3. 交互与边界

- 空配置合法，表示使用右侧/下方展示的默认公开源。
- 非 HTTP(S)、畸形 URL 等由后端返回 `ok:false`，前端显示具体错误。
- `only_custom` 无任何完整 URL 和前缀时显示 `deps.mirror_only_bad`。
- 输入最后一个非空完整 URL 时自动追加下一空行，减少“添加”按钮占用；空行不会写入配置。
- 前缀 textarea 使用真实换行拆分，兼容 CRLF/LF。
- 默认源只读，以 `<code>` 列表展示。
- 文案全部使用现有 `deps.mirror_*` 与公共 i18n key；本轮未在 JS 中新增硬编码中文。
- 不增加常驻轮询或动画；只有进入页面读取一次、用户主动保存/测试时发请求。

## 4. 后端配合与部署注意

1. 后端继续维持 `web/CONTRACT.md` 约定的三个接口和 HTTP 200 + `ok` 结果语义。
2. 前端使用响应中的 `deps`、`builtin`、`prefix`、`mirrors`、`only_custom`；字段变更需同步更新契约。
3. 后端需将 `web/` 同步到 `crates/vca-gui/src/web/` 后重新编译内置兜底资源；本前端任务按边界没有修改 `crates/`。
4. `locales/zh-CN.json` 与 `locales/en-US.json` 必须保留现有 `deps.mirror_*` 键。

## 5. 自检结果

- `node --check web/app.js`：通过。
- `python -m py_compile scripts/smoke.py`：通过。
- `python scripts/smoke.py`：**58/58 全部通过**。
- 完整质量关：
  - `cargo fmt --all -- --check`：通过；
  - `cargo clippy --workspace --all-targets -- -D warnings`：通过，0 告警；
  - `cargo test --workspace`：422 通过、0 失败、2 ignored。
- 真实 HTTP 镜像配置往返已由 smoke 项覆盖：3 项默认源、保存规范化、同进程即时回读、4 类非法输入、重置。

## 6. 设计 skills 与模型运行记录

任务要求使用：

- `https://github.com/Leonxlnx/taste-skill`
- `https://github.com/pbakaus/impeccable`

当前团队运行没有留下可核验的安装目录、固定 commit 或使用记录，因此本文件**不宣称已经安装或采用**这两个第三方 skill。前端实际遵循的可核验原则是：原生静态资源、紧凑信息层级、明确输入/只读内容、即时状态反馈、窄屏适配、无重型依赖。

团队运行请求的模型路由为 `xindu-kimi/kimi-k2.7-code`，但运行器持续显示：

```text
subagent 通道的模型继承父会话，团队/角色模型设置不生效
```

因此只能证明“请求了该路由”，不能证明实际执行通道采用该模型；此项列为待确认，不能虚报。

## 7. 已知问题与待确认

- 第三方设计 skill 的安装和固定 commit：没有可核验证据，待确认。
- 团队运行的实际模型：运行器警告覆盖无效，待平台侧确认。
- 公网镜像真实大文件下载受网络影响；前端仅负责配置和轻量探测，不宣称已下载完整公网文件。
- 最终视觉观感仍建议在目标 Windows/WebView2 真机上人工确认；自动化已覆盖接口和静态语法，不能替代人眼。
