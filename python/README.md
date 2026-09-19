# python/  算法侧 Worker（子进程）

本目录承载**重算法与生态依赖密集**的部分（策划书 5.1 选型结论）：

- 语音转写：`faster-whisper`（默认）/ `whisper.cpp`（极低配备选）
- 文档生成：`python-docx`（Word） `LibreOffice headless`（PDF）
- 截图去重：感知哈希（`Pillow` / `imagehash`）

## 为什么单独放在 Python

Rust 宿主负责调度、CLI、插件管理与录制编排（体积小、内存低、并发安全）；
Python 只做重算法，由宿主**按需拉起、用完释放**，从而规避其「内存大、启动慢」的缺点。

## 通信方式

宿主通过 **JSON-RPC 2.0 over stdio** 调用本 Worker，协议定义见 `crates/vca-ipc`。

```
宿主 --(stdin, JSON-RPC)--> vca_worker
宿主 <--(stdout, JSON-RPC)-- vca_worker
```

## 当前状态

**占位骨架**：仅提供可启动的 RPC 循环与 `describe` 响应，
各算法函数均为 TODO，未实现真实逻辑。

## 运行

```bash
# 以模块方式启动（供宿主拉起）
python -m vca_worker --stdio

# 自检
python -m vca_worker --selftest
```

## 依赖

见 `requirements.txt`。**注意**：依赖安装受网络与权限影响，
在受限环境下可先不安装框架骨架不依赖这些包即可运行。
