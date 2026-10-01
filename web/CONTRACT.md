# 前端接口契约（`web/` ↔ Rust 后端）

> 本文是**前后端分离的接口约定**。负责界面重构的 agent 只需要读这一份文档 +
> `web/` 目录，**不需要读任何 Rust 代码**。

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
