# config/  配置目录

## 文件说明

| 文件 | 用途 | 是否含密钥 |
|---|---|---|
| `settings.example.yaml` | 主配置示例（录制/转写/文档/清理/性能/界面） | 否，可入库 |
| `timetable.example.yaml` | 时间表示例（作息骨架） | 否，可入库 |
| `schedule.example.yaml` | 课程表示例（课程填充 + 录制勾选） | 否，可入库 |

## 实际配置的存放位置

示例文件只是模板。程序运行时读取的是：

```
config/
  global.yaml                         全局配置
  timetable/
    current.yaml                      当前生效时间表
    archive/{yyyy-ww}/*.yaml          时间表全量版本化存档
  schedule/
    current.yaml                      当前生效课程表
    archive/{yyyy-ww}/*.yaml          课表全量版本化存档
  profiles/{profile}/
    settings.yaml                     该教师的配置
    secrets.env                       该教师的密钥（禁止入库）
```

## 两条硬规则

1. **密钥零入库**：`secrets.env` 已被 `.gitignore` 排除；
   所有 token / webhook / password 走环境变量或该文件。
2. **零预置**：程序出厂不含任何真实学校作息、课程名、教师名。
   示例文件中的「课程一 / 教师甲 / 教室一」均为虚构占位。

## 存档策略

时间表与课表**每次保存都生成新版本**，旧版本不覆盖，
按周（`yyyy-ww`）归档，**不按学期归档**，永久保留，不自动删除。
