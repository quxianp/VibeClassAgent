# 前端接口契约（`web/` ↔ Rust 后端）

> 本文是**前后端分离的接口约定**。负责界面重构的 agent 只需要读这一份文档 +
> `web/` 目录，**不需要读任何 Rust 代码**。

> 👉 **第一次接手请先看 [`README.md`](./README.md)** —— 那里有"怎么跑起来看效果"
> "改哪不改哪""怎么知道自己做对了"，看完再来查这里的接口细节。

---

## 1. 分工边界

| 目录 | 谁负责 | 说明 |
|---|---|---|
| `web/` | **前端 agent** | 界面全部在这里。改完刷新页面即见，**不需要重新编译 Rust** |
| `crates/` | 后端 | 业务逻辑、接口实现、数据读写 |
| `crates/vca-gui/src/web/` | 后端 | 内置兜底资源（`include_str!`）。**前端不要改这里** |

### 资源查找顺序（运行时）

1. `VCA_WEB_DIR` 环境变量指定的目录（脚本/CI 用）
2. 仓库根 / exe 往上 4 层内的 `web/`
3. **exe 同级的 `web/`** —— 发布形态：换掉这个目录就等于更新前端
4. 都没有 → 用编进 exe 的内置版本

**所以 `web/index.html` 一旦存在，它就会盖住内置版本。** 前端改这里就生效。

### 后端同步规则

前端资源改了以后，后端需要把 `web/` 的内容同步一份到
`crates/vca-gui/src/web/`（内置兜底），再重新编译。这一步**由我（后端）负责**，
前端 agent 不用管 —— 它只管改 `web/` 下的文件。

---

## 2. 怎么跑起来看效果

```powershell
# 起服务（不自动开浏览器），然后手动访问打印出来的地址
target\release\vca.exe gui --no-open --port 4970

# 指定别的前端目录（可选）
$env:VCA_WEB_DIR = 'D:\VibeClassAgent\web'
target\release\vca.exe gui --port 4970
```

服务只绑 `127.0.0.1`，URL 带一次性令牌：`http://127.0.0.1:4970/?t=<TOKEN>`。
**所有 `/api/*` 请求都必须带 `?t=<TOKEN>`**，否则返回 403。

> 令牌从日志里取：`data/logs/ui.log` 里搜 `?t=`。

---

## 3. 通用约定

### 请求

- 所有接口都是 `GET` 或 `POST`，「带 body」的 POST 一律 `Content-Type: application/json`。
- 令牌通过 query 传：`?t=<TOKEN>`（与其它 query 参数并存，如 `?t=xxx&lines=200`）。
- **注意**：静态资源（`.html/.css/.js`）**不需要**令牌，只有 `/api/*` 需要。

### 响应

统一是 JSON：

```jsonc
// 成功
{ "ok": true, ... }

// 失败
{ "ok": false, "error": "人类可读的中文错误说明" }
```

- HTTP 状态码：`200` 成功、`403` 令牌不对、`404` 路径不存在、`500` 服务端异常。
- **前端应当始终检查 `ok` 字段**，不要只看状态码 —— 业务失败也是 `200`。
- 中文错误信息可以直接展示给用户（界面语言是中文）。

### 缓存

所有静态资源都带 `Cache-Control: no-store`。**不要依赖浏览器缓存**做版本管理。

---

## 4. 接口清单

### 4.1 状态与依赖

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/status` | 总状态：本地转写是否就绪、模型、推送渠道等 |
| GET | `/api/deps` | **依赖自检**。见下方结构 |
| POST | `/api/deps/fetch` | 触发下载补齐。body 可带 `{"only":["ffmpeg"]}` 只补指定项 |
| GET | `/api/deps/fetch` | 查询下载进度（前端轮询这个） |
| POST | `/api/deps/dismiss` | 用户点「稍后」——可选项从此不再提示 |
| GET | `/api/deps/mirrors` | 读**下载源（镜像）**配置。见 4.1.1 |
| POST | `/api/deps/mirrors` | 保存下载源配置 |
| POST | `/api/deps/mirrors/test` | 测一个地址通不通（轻量探测，不下载文件） |

`/api/deps` 返回：

```jsonc
{
  "ok": true,
  "tools_root": "D:\\VibeClassAgent\\tools",
  "items": [
    {
      "id": "ffmpeg",              // ffmpeg | whisper | model
      "label": "ffmpeg",           // 可直接展示的名字
      "consequence": "缺少它无法录制", // 缺了的后果，直接展示给用户
      "required": true,            // true=必需（每次提示） false=可选（只提示一次）
      "ready": false,
      "reason": "文件不存在",        // ready=false 时才有意义
      "path": "..."
    }
  ],
  "missing": 2,          // 缺几项
  "need_notice": 2,      // **这次该弹窗提示几项**（0 = 不要弹）
  "notice_required": true, // 有没有必需项缺失
  "all_auto_fixable": true // 是否都能自动下载
}
```

> **`need_notice` 是关键**：它已经替你算好了"这次要不要打扰用户"。
> 前端只要 `if (st.need_notice) 弹窗`，不要自己判断 `missing.length`。

`/api/deps/fetch`（GET）返回：

```jsonc
{ "ok": true, "running": true, "message": "正在下载 ffmpeg（12.3 MB / 98.1 MB）", "error": null }
```

前端轮询间隔建议 **800ms**，`running` 变 `false` 后重新拉一次 `/api/deps` 看真实结果。

#### 4.1.1 下载源（镜像）自定义

用户已确认的默认策略是：**未填写时保留现有公开源列表**，不新增单一强制镜像：

- ffmpeg：gyan.dev → GitHub；
- whisper.cpp：GitHub → ghfast；
- tiny 模型：hf-mirror → HuggingFace。

校园网 / 单位内网里这些公开地址可能一个都连不上，因此用户可以在界面上填自己的
镜像地址。**实际下载顺序严格是：依赖专属自定义地址 → 全局前缀拼接地址 → 默认公开源**；
勾选 `only_custom` 后不再追加最后一组默认公开源。后端不会用硬编码地址覆盖用户输入。

配置入口采用“前端表单 + profile 配置文件”组合，不另设环境变量：URL 不是凭据，且
按教师 profile 持久化更符合本项目多用户隔离。对应文件：
`<config_root>/profiles/<profile>/settings.yaml`。

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/deps/mirrors` | 读当前配置 + 内置源（只读展示） |
| POST | `/api/deps/mirrors` | 保存配置 |
| POST | `/api/deps/mirrors/test` | 测一个地址通不通 |

`GET /api/deps/mirrors` 返回：

```jsonc
{
  "ok": true,
  "prefix": ["https://ghfast.top/"],          // 全局加速前缀，可空
  "mirrors": { "ffmpeg": ["https://my.mirror/ffmpeg.zip"] }, // 每个依赖自己的地址
  "only_custom": false,                        // true = 不试内置地址
  "builtin": { "ffmpeg": ["https://www.gyan.dev/..."], "whisper": [], "model": [] },
  "deps": [                                    // 拿它渲染表单：一项一组输入框
    { "id": "ffmpeg", "label": "ffmpeg", "required": true, "consequence": "缺少它无法录制" }
  ]
}
```

> `mirrors` 的键必须是 `deps[].id`（`ffmpeg` / `whisper` / `model`）。**没填的依赖不要写进
> 请求体**，写了空数组等价于没填。未知 id、非数组、数组内非字符串都会返回 `ok:false`，
> 不会静默保存成一个下载器永远不读取的“假成功”配置。
>
> `mirrors` 里填的是**完整文件 URL**；`prefix` 才是拼在默认 URL 前的加速前缀。
> `prefix` 带不带结尾 `/` 都可，保存时会规范为恰好一个 `/`；前缀不能带 `?` 或 `#`，
> 这类特殊镜像请改为给每个依赖填写完整 URL。

`POST /api/deps/mirrors` 请求体（**整体覆盖式保存**，不是增量）：

```jsonc
{
  "prefix": ["https://ghfast.top/"],
  "mirrors": { "ffmpeg": ["https://my.mirror/ffmpeg.zip"] },
  "only_custom": false
}
```

三种结果，**都用 `ok` 判断，不要用 HTTP 状态码**：

```jsonc
{ "ok": true,  "message": "下载源已保存", "prefix": [], "mirrors": {}, "only_custom": false }
```

```jsonc
{ "ok": false, "error": "前缀镜像「xxx」不合法：必须以 http:// 或 https:// 开头", "field": "prefix" }
```

```jsonc
{ "ok": false, "error": "勾了「只用自定义源」但一个地址都没填，这样必然下载失败。请取消勾选，或至少填一个地址。" }
```

`field` 会等于不合法的那个字段名（`prefix` 或依赖 id），可以拿来把出错那一格标红。
保存成功后返回的是**回读磁盘的真实值**，请用它刷新界面，不要沿用本地状态
（YAML 可能把空值吃掉）。

`POST /api/deps/mirrors/test` 请求 `{"url": "https://my.mirror/ffmpeg.zip"}`：

```jsonc
{ "ok": true,  "status": 200, "kind": "file", "size": 102906624, "message": "文件，98.1 MB" }
```

```jsonc
{
  "ok": false,
  "error": "连不上：connection refused",
  "hint": "确认地址能在浏览器里直接下载文件（不是网页）。如果是内网镜像，确认这台机器能访问它。"
}
```

> **测一个地址是网络请求，会阻塞最多 8 秒**。界面上要给出「正在连…」的反馈，
> 别让按钮点了没动静。它是轻量探测（HEAD，拿不到就退化成极小范围 GET），
> **不会下载整个文件**。

保存后**无需重启程序**：下载时会重新读配置。但已经发起的那次下载不受影响。

### 4.2 配置

| 方法 | 路径 | 说明 |
|---|---|---|
| GET/POST | `/api/config/general` | 通用设置 |
| GET/POST | `/api/config/llm` | 模型接口配置（Key 不会回显明文） |
| GET/POST | `/api/config/push` | 推送配置 |
| GET | `/api/providers` | 可选的模型供应商预设 |
| POST | `/api/models` | 拉取某供应商的可用模型列表 |
| POST | `/api/push/test` | 发一条测试消息 |
| POST | `/api/push/discover` | 开始探测可推送的会话 |
| GET | `/api/push/discover` | 查询探测进度 |
| POST | `/api/push/discover/stop` | 停止探测 |

### 4.3 课表与时间表

| 方法 | 路径 | 说明 |
|---|---|---|
| GET/POST | `/api/schedule` | 课表。`GET` 返回 `{ok, exists, entries, teachers, class_slots}` |
| GET/POST | `/api/timetable` | 作息时间表 |
| POST | `/api/import/classisland` | 导入 ClassIsland 课表 |

> `exists: false` 表示还没建课表 —— 前端应走"引导用户新建"的界面，
> 而不是显示一张空表格。

### 4.4 作业与录制

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/jobs` | 作业列表。每项含 `id/course/date/start/state/last_error/docx/...` |
| POST | `/api/jobs/run` | 触发处理某个作业 |
| GET | `/api/jobs/run` | 查询处理进度 |
| GET | `/api/daemon/status` | 守护进程状态：`{running, dry_run, uptime, uptime_secs, last_error}` |
| POST | `/api/daemon/start` | 启动守护。body 可带 `{"dry_run":true}` |
| POST | `/api/daemon/stop` | 停止守护 |
| POST | `/api/clean/preview` | 清理预览（**只算不删**） |

`job.state` 的取值（前端做状态标记/配色用）：

```
Pending → Recorded → Transcribed → Extracted → Linked → DocReady → Pushed → Cleaned
                                                                  ↘ Failed
```

### 4.5 机器人（QQ）

| 方法 | 路径 |
|---|---|
| GET | `/api/bot/napcat` |
| POST | `/api/bot/napcat/install` |
| POST | `/api/bot/napcat/start` |
| POST | `/api/bot/napcat/stop` |
| POST | `/api/bot/napcat/open` |
| POST | `/api/bot/napcat/configure` |
| POST | `/api/detect-bot` |

### 4.6 其它

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/i18n` | **界面文案**。见第 5 节 |
| GET | `/api/logs` | 日志尾巴。`?lines=200` 控制行数 |
| POST | `/api/logs/clear` | 清空日志缓冲 |
| POST | `/api/quit` | 退出程序 |

---

## 5. 文案（i18n）

**不要在 JS 里硬编码中文。** 所有文案走 `/api/i18n`：

```jsonc
{
  "ok": true,
  "lang": "zh-CN",
  "strings": {
    "deps.title": "缺少运行依赖",
    "deps.lead": "...",
    "common.save": "保存"
    // ... 扁平的点号键
  }
}
```

- 后端已经产出了 **扁平的点号键**（不是嵌套对象），直接 `L['deps.title']` 取。
- 语言文件在 `locales/zh-CN.json` 与 `locales/en-US.json`。
- **新增文案请找我加**（后端负责语言文件），不要在 JS 里写死 —— 否则英文版会缺。

现有语言文件顶层分组：`web.` 前缀会被剥掉，所以 JS 里看到的是 `deps.title`
而不是 `web.deps.title`。

**已为「下载源（镜像）」预置好文案**（在 `deps.*` 下，英文版同步有了），直接用：

| 键 | 用途 |
|---|---|
| `deps.mirror_title` | 面板标题「下载源（镜像）」 |
| `deps.mirror_lead` | 面板说明（为什么需要填） |
| `deps.mirror_adv` | 折叠「展开高级设置」 |
| `deps.mirror_item` | 每个依赖下面的字段名「下载地址（一行一个，按顺序试）」 |
| `deps.mirror_url_ph` | 地址输入框 placeholder |
| `deps.mirror_prefix` / `deps.mirror_prefix_ph` / `deps.mirror_prefix_note` | 全局加速前缀那一格 |
| `deps.mirror_test` / `deps.mirror_testing` | 「测一下」按钮与进行中 |
| `deps.mirror_test_ok` / `deps.mirror_test_fail` | 探测结果的短标签 |
| `deps.mirror_only` / `deps.mirror_only_note` / `deps.mirror_only_bad` | 「只用我填的地址」复选框 |
| `deps.mirror_builtin` / `deps.mirror_builtin_note` | 只读的内置地址展示区 |
| `deps.mirror_save` / `deps.mirror_saved` / `deps.mirror_save_failed` | 保存按钮与结果 |
| `deps.mirror_reset` | 「清空（用默认）」 |
| `deps.mirror_need_url` | 一个都没填时的本地提示 |

---

## 6. 已有的前端约定（重构时可以改，但要知道现状）

- 纯 **vanilla JS**，无框架、无构建步骤。`app.js` 目前是单文件 111 KB。
- `index.html` 里只有骨架，内容全由 JS 渲染。
- CSS 变量的主题色定义在 `style.css` 顶部（`--bg-side`、`--accent` 等）。
- 现有类名约定用 `.btn.primary`（**不是** `.btn-primary`）。
- 取文案的函数是 `t('key')`，请求封装是 `api(path, body?)`（自动补令牌）。

### 如果你想引入构建步骤

可以，但要满足两条：

1. **产物必须落在 `web/` 下且是浏览器能直接跑的形式**（不要让我在部署时跑 `npm build`）；
2. `web/index.html` 必须是入口 —— 后端只认这个文件名做"前端存在"的判据。

### 后端会替你守住的事

- 令牌校验（前端只要带上 `?t=`）；
- 路径穿越（前端不能通过 URL 读到 `web/` 以外的文件）；
- `no-store` 缓存头；
- 正确的 `Content-Type`（按扩展名，认不出的给 `application/octet-stream`）。

---

## 7. 联调与验收

后端提供 `scripts/smoke.py`（58 项，打真实 HTTP 接口）。
**前端重构后请确保它仍然全过**：

```powershell
python scripts\smoke.py
```

它覆盖了令牌保护、配置读写往返、课表推导、导入、守护进程起停等。
如果重构把某个接口的调用方式改了，这个脚本会红 —— 那就找我商量，
**不要**为了让脚本过而绕过接口。
