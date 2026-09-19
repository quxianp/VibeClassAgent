# 插件：example.echo（示例）

| 项 | 值 |
|---|---|
| 类型 | `pusher` |
| 插件 id | `example.echo` |
| 状态 | **已实现，可以直接跑** |

这是本项目**唯一一个功能完整的插件**，其余八个目录仍只有清单与说明。
写它的目的不是回显，而是证明「插件架构」不是空话：
宿主能真的把它拉起来，走完 `describe` / `init` / `health_check` / `run` / `cleanup`
五个方法的 JSON-RPC 往返。

## 它做什么

把 `run` 收到的参数原样回显，并返回一个 `success: true` 与生成的 `messageId`。
功能上没什么用，但**协议部分是真的**。

## 怎么构建

插件是独立进程，源码在 `crates/vca-plugin-example/`：

```bash
cargo build --release -p vca-plugin-example
mkdir -p plugins/example-echo/bin
cp target/release/vca-plugin-example.exe plugins/example-echo/bin/
```

> `entry` 是**相对插件目录**的路径，所以必须放在 `bin/` 下并在 manifest 里
> 写成 `bin/vca-plugin-example.exe`。写成裸命令名是找不到的 ——
> 宿主用的是 `plugin_dir.join(entry)`。

## 怎么验证

```bash
vca plugin list          # 应当能看到 example.echo
vca plugin info example.echo
```

## 自己写插件

插件**不限语言**。宿主与插件之间只有两条约定：

1. 插件是独立进程，从 **stdin** 逐行读 JSON-RPC 请求，把响应逐行写到 **stdout**；
2. 实现五个方法：

| 方法 | 用途 | 返回 |
|---|---|---|
| `describe` | 报告身份与能力 | `{id, kind, version, api_version, config_schema, capabilities}` |
| `init` | 读配置、准备资源（凭据从**环境变量**取） | `{ok: true}` |
| `health_check` | 自检 | `{healthy: bool, message}` |
| `run` | 干活 | 视类型而定，推送类返回 `{success, messageId}` |
| `cleanup` | 释放资源 | `{ok: true}` |

三条容易踩的规矩：

- **stdout 只能走协议**。任何调试输出必须写 stderr ——
  往 stdout 多打一行，宿主就会当成非法 JSON 而判定插件故障；
- **凭据一律读环境变量**，不要从 `params` 里塞明文，更不要写进 manifest；
- `api_version` 的**大版本**必须与宿主一致，否则会被拒绝加载。

Python / Go / Node / C++ 都能写，只要会用标准输入输出就行。
这也是当初选「进程 + stdio」而不是动态库的原因：语言无关、天然隔离、崩溃不连累宿主。
