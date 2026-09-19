# 插件：doc-generator

**类型**：`doc`
**插件 id**：`doc`
**状态**：占位（框架骨架，未实现）

## 职责

文档生成（策划书 5.4）。Word + PDF，本地保存。

## 实现状态

| 项 | 状态 |
|---|---|
| manifest.toml | 已就位 |
| README | 已就位 |
| 可执行体 `vca-plugin-doc` | **待实现** |
| checksums.json | **待生成** |
| signature.sig | 可选，待定 |

## 待办

1. 实现插件可执行体，遵循 `vca-ipc` 定义的 JSON-RPC 方法：
   `init` / `run` / `cleanup` / `health_check` / `describe`；
2. 生成 `checksums.json`；
3. 补齐 `[config]` 段的 JSON Schema（供 CLI 生成配置表单）；
4. 若涉及第三方协议或敏感权限，填写 `risk_note` 并在安装时向用户确认。
