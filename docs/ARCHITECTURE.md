# 架构说明

## 1. 分层

```

 交互层   CLI (clap)  配置  日志  小任务栏图标            

 调度层   时间表/课表驱动  时段触发  任务队列  多 profile  

 插件层   recorder  transcriber  extractor  linker      
          doc  pusher  cleaner   （JSON-RPC，可插拔）      

 核心服务 事件总线  配置  文件  存档  密钥  状态机       

 平台层   Windows 采集  进程  计划任务  负载探测  自检    

```

## 2. crate 职责与依赖

```
vca-cli  > vca-core        （领域模型、配置、状态机、路径）
           > vca-ipc         （协议契约）
           > vca-plugin-host （插件宿主，依赖 vca-ipc）
           > vca-platform    （平台适配，依赖 vca-core）
```

依赖方向单向向下：**cli  {core, ipc, plugin-host, platform}**，
`core` 与 `ipc` 不反向依赖任何上层 crate，便于单元测试与替换。

## 3. 时间表与课程表分离（核心设计）

| 概念 | 定义 | 可复用性 |
|---|---|---|
| **时间表** | 作息骨架：第几节几点到几点、课间与休息段 | 多份课程表可复用同一时间表 |
| **课程表** | 骨架上的填充：某天某节什么课、哪位老师、是否录制 | 可按单双周拆分为多份 |

两者组合后生成**录制计划**。这一分离使系统能适配任意学校的作息差异，
且时间表可「用户自主配置」或「从 ClassIsland 导入」，或二者混合。

## 4. 数据流

```
时间表（自主配置 / 导入）  +  课程表（手填 / CSV / ClassIsland）
      
      
  录制计划（仅勾选课）
      
        上课时段
  recorder  raw/ + audio/ + screenshots/   （按 profile 隔离）
      
        处理窗口（午休 / 放学后 / 晚自习后，默认串行）
  transcriber  extractor  linker  doc-generator (Word/PDF)
      
      
  pusher（用户自选渠道） 推送成功 t0
      
      
  cleaner：expireAt = t0 + 72h  到期二次校验  物理删除
```

## 5. 作业状态机

```
Pending  Recorded  Transcribed  Extracted  Linked  DocReady  Pushed  Cleaned
                                                                    
                              任意状态  Failed  Recorded 
                                                    （重试）
```

关键约束：

- **`Pushed` 是 72 小时倒计时的唯一起点**（`push_succeeded_at`）；
- 推送未成功则**不进入倒计时、不删除**；
- 状态迁移合法性由 `JobState::can_transition_to` 强制校验。

## 6. 插件机制

- 插件以**独立进程**运行（默认 `process` 模式），通过 **JSON-RPC 2.0 over stdio** 通信；
- 统一方法：`init` / `run` / `cleanup` / `health_check` / `describe`；
- `api_version` 大版本不匹配  拒绝加载；
- 优点：语言无关（C++/Python/Go/Node 均可写插件）、天然沙箱、崩溃不影响宿主。

详见 `docs/PLUGIN_SDK.md`。

## 7. 性能设计（低配卡顿机基准）

| 手段 | 说明 |
|---|---|
| 极简默认 | 720p@8fps、CRF30、tiny 模型、截图 180s |
| 自适应降档 | 采集前与录制中每 30s 复评负载，高负载降 fps/码率 |
| 硬编优先 | QSV/NVENC/AMF，失败降级 x264 并同步降分辨率 |
| 错峰转写 | 严格放在处理窗口，不与录制重叠，默认串行（`max_concurrent: 1`） |
| 子进程用完即退 | Python Worker 不常驻 |
| 插件按需加载 | 未启用插件不加载 |

目标指标见策划书 7.1。

## 8. 多教师共用

- 身份：默认 Windows 登录账户；同账户多人用 `--profile` 显式切换；
- 数据/配置隔离：`data|config/profiles/{profile}/`；
- 课表条目绑定任课老师  自动路由到其 profile；
- profile 目录 ACL 限本人。
