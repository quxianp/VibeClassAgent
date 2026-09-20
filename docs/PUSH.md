# 推送配置与本地机器人部署

VCA 的推送目标不是某个平台的 SDK，而是**一个 HTTP 端点**。
最常见的部署是「NapCat（登录 QQ）+ VCA（发消息）」。

---

## 一、最短可用路径：NapCat

1. 下载 [NapCat](https://github.com/NapNeko/NapCatQQ)，按它的文档登录一个 QQ 号
   （**建议用小号**：这属于第三方协议登录，有账号风险）
2. 在 NapCat 配置里打开 **HTTP 服务端**，记下端口（默认 `3000`）与 access token
3. 把这个 QQ 号拉进你要接收课堂纪要的群
4. 配 VCA 的 `config/profiles/default/settings.yaml`：

```yaml
push:
  provider: "onebot"
  endpoint: "http://127.0.0.1:3000"
  target: "你的群号"
  target_type: "group"
```

token 不要写进这个文件，放 `secrets.env`（同目录）或环境变量：

```
VCA_PUSH_TOKEN=你的access_token
```

5. **验证**：

```powershell
vca.exe debug push-test
```

它会把实际使用的配置原样打出来（token 打码），并真的发一条测试消息。
`config/profiles/default/` 就是配置目录；交互界面主菜单「2 配置推送」也能配。

---

## 二、AstrBot 放在哪

[AstrBot](https://github.com/AstrBotDevs/AstrBot) 是**对话机器人框架**，
它自己也需要一个 OneBot 实现（同样是 NapCat）才能上 QQ。

所以三者的关系是：

```mermaid
flowchart LR
    QQ["QQ 群"] <--> N["NapCat"]
    N <-->|"WebSocket"| A["AstrBot<br/>聊天机器人"]
    N <-.->|"HTTP API"| V["VCA<br/>只发不收"]
```

**关键点：NapCat 可以同时开 HTTP API 与 WebSocket。**
AstrBot 走 WebSocket、VCA 走 HTTP，两者互不干扰 ——
**VCA 不需要经过 AstrBot 中转**，直连同一个 NapCat 就行，链路更短、少一层故障点。

什么时候才需要经过 AstrBot：想让课堂纪要触发 AstrBot 里的某个工作流。
那就反过来 —— 在 AstrBot 侧挂一个 HTTP 接口，VCA 用 `webhook` 通道打过去。

---

## 三、其它渠道

| provider | 需要什么 | 备注 |
|---|---|---|
| `wecom` | 群机器人 Webhook URL | **最省事**，不装任何东西 |
| `onebot` | NapCat 地址 + token + 群号 | 功能最全，能发文件 |
| `qq` | 开放平台 AppID / Secret + 群号 | 官方合规，但需审核 |
| `serverchan` | SendKey | 推到微信，只要一个 key |
| `webhook` | 一个 HTTP 地址 | 对接自己的服务 |
| `wechat-personal` | 第三方协议服务地址 | **有账号风险**，自行评估 |

---

## 四、排查顺序

`vca debug push-test` 报的是**原始错误**，对着看：

| 现象 | 原因 |
|---|---|
| `connection refused` | NapCat 没起来，或端口不对 |
| `403` | token 与 NapCat 里设的不一致 |
| `retcode=...` + 一段文字 | NapCat 的业务错误原文，通常是机器人不在该群 / 群号写错 |
| 显示成功但群里没消息 | `target` 填成了 QQ 号；发群必须 `target_type: group` |
| 文字到了、文件没到 | OneBot 上传文件要求路径在 NapCat 可访问范围内 —— 两者装同一台机器最省事 |

> 文件是「尽力而为」：文本送达就算这次推送成功，文件失败只记一条警告。
> 这是**故意的** —— 否则文件传不上去会触发「不删除本地录像」的策略，把磁盘占满。
