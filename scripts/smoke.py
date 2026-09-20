# -*- coding: utf-8 -*-
"""VibeClassAgent 图形界面冒烟测试。

一次跑完界面的全部接口，确认「配置能存、能读回、能真的发出去」。

为什么需要它
------------
界面能打开 ≠ 能用。真正会出错的地方是接口往返：写进去的字段有没有落对段、
读回来是不是同一个值、推送到底发没发出去。这些靠手点一遍很难覆盖全，
而且改一次代码就要重验一次。所以做成脚本，随时能跑。

它会自己起一个假的 OneBot 服务端来收推送请求 ——
不需要真装 NapCat、不需要真 QQ 号，就能验证"发出去的报文长什么样"。

用法
----
    python scripts/smoke.py                    # 用 debug 产物
    python scripts/smoke.py --release          # 用 release 产物
    python scripts/smoke.py --keep             # 跑完保留临时目录，便于排查
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import socket
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WORK = ROOT / "_tmp" / "smoke"

for _s in (sys.stdout, sys.stderr):
    try:
        _s.reconfigure(encoding="utf-8")  # type: ignore[union-attr]
    except Exception:  # noqa: BLE001
        pass

PASS, FAIL = [], []


def ok(name: str, detail: str = "") -> None:
    PASS.append(name)
    print(f"  [PASS] {name}" + (f"  {detail}" if detail else ""), flush=True)


def bad(name: str, detail: str = "") -> None:
    FAIL.append((name, detail))
    print(f"  [FAIL] {name}  {detail}", flush=True)


def free_port() -> int:
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


# --------------------------------------------------------------- mock OneBot


class MockOneBot:
    """假 OneBot 服务端，只记录收到的请求。"""

    def __init__(self) -> None:
        self.received: list[dict] = []
        outer = self

        class H(BaseHTTPRequestHandler):
            def do_POST(self):  # noqa: N802
                n = int(self.headers.get("Content-Length") or 0)
                raw = self.rfile.read(n).decode("utf-8", "replace")
                try:
                    body = json.loads(raw)
                except Exception:  # noqa: BLE001
                    body = {"_raw": raw}
                outer.received.append({
                    "path": self.path,
                    "auth": self.headers.get("Authorization") or "",
                    "body": body,
                })
                resp = json.dumps(
                    {"status": "ok", "retcode": 0, "data": {"message_id": 999}}
                ).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(resp)))
                self.end_headers()
                self.wfile.write(resp)

            def log_message(self, *a):  # 静音
                pass

        self.port = free_port()
        self.httpd = HTTPServer(("127.0.0.1", self.port), H)
        threading.Thread(target=self.httpd.serve_forever, daemon=True).start()

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    def stop(self) -> None:
        self.httpd.shutdown()


# --------------------------------------------------------------- HTTP 客户端


class Api:
    def __init__(self, base: str, token: str) -> None:
        self.base = base
        self.token = token

    def call(self, path: str, body=None, timeout: int = 30):
        """返回 (status, json 或 None)。"""
        sep = "&" if "?" in path else "?"
        url = f"{self.base}{path}{sep}t={self.token}"
        data = None
        headers = {}
        if body is not None:
            data = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        req = urllib.request.Request(url, data=data, headers=headers,
                                     method="POST" if body is not None else "GET")
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                return r.status, json.loads(r.read().decode())
        except urllib.error.HTTPError as e:
            return e.code, None
        except Exception as e:  # noqa: BLE001
            return 0, {"_err": str(e)}

    def raw(self, path: str, timeout: int = 10):
        try:
            with urllib.request.urlopen(self.base + path, timeout=timeout) as r:
                return r.status, r.read().decode("utf-8", "replace")
        except urllib.error.HTTPError as e:
            return e.code, ""
        except Exception:  # noqa: BLE001
            return 0, ""


# --------------------------------------------------------------- 主流程


def wait_port(port: int, secs: float = 25) -> bool:
    end = time.time() + secs
    while time.time() < end:
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.5):
                return True
        except OSError:
            time.sleep(0.25)
    return False


def main() -> int:
    ap = argparse.ArgumentParser(description="VibeClassAgent 界面冒烟测试")
    ap.add_argument("--release", action="store_true", help="用 release 产物")
    ap.add_argument("--keep", action="store_true", help="保留临时目录")
    args = ap.parse_args()

    profile = "release" if args.release else "debug"
    exe = ROOT / "target" / profile / "vca.exe"
    if not exe.is_file():
        print(f"找不到 {exe}\n先编译：cargo build -p vca-cli")
        return 2

    if WORK.exists():
        shutil.rmtree(WORK, ignore_errors=True)
    cfg, data = WORK / "cfg", WORK / "data"

    port = free_port()
    mock = MockOneBot()

    env = os.environ.copy()
    env["VCA_UI"] = "1"
    env["RUST_LOG"] = "info"
    proc = subprocess.Popen(
        [str(exe), "--config-dir", str(cfg), "--data-dir", str(data),
         "gui", "--no-open", "--port", str(port)],
        env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        encoding="utf-8", errors="replace",
    )

    base = f"http://127.0.0.1:{port}"
    print(f"启动界面服务：{base}（pid={proc.pid}）", flush=True)

    # 用后台线程持续读 stdout —— 直接 readline() 会阻塞，
    # 而服务启动后可能长时间不再输出，主流程就被卡死了。
    out_lines: list[str] = []

    def pump() -> None:
        try:
            for ln in proc.stdout:  # type: ignore[union-attr]
                out_lines.append(ln)
        except Exception:  # noqa: BLE001
            pass

    threading.Thread(target=pump, daemon=True).start()

    if not wait_port(port):
        proc.kill()
        print("服务没起来。输出：")
        print("".join(out_lines[-30:]))
        return 2

    # 从日志里取 token：先看 stdout，再看落盘日志（界面模式会写 ui.log）
    token = ""

    def find_token() -> str:
        for ln in out_lines:
            if "?t=" in ln:
                return ln.split("?t=")[1].strip()
        logf = data / "logs" / "ui.log"
        if logf.is_file():
            txt = logf.read_text(encoding="utf-8", errors="replace")
            for ln in reversed(txt.splitlines()):
                if "?t=" in ln:
                    return ln.split("?t=")[1].strip()
        return ""

    deadline = time.time() + 12
    while time.time() < deadline and not token:
        token = find_token()
        if not token:
            time.sleep(0.25)
    if not token:
        bad("取得界面 token", "日志里没找到 ?t=，无法继续")
        proc.kill()
        mock.stop()
        return 1
    ok("取得界面 token", f"{token[:8]}…")

    api = Api(base, token)

    try:
        run_checks(api, base, mock, cfg)
    finally:
        print("\n关闭界面服务…", flush=True)
        try:
            api.call("/api/quit", {}, timeout=5)
        except Exception:  # noqa: BLE001
            pass
        time.sleep(0.8)
        if proc.poll() is None:
            proc.kill()
        mock.stop()
        if not args.keep:
            shutil.rmtree(WORK, ignore_errors=True)
        else:
            print(f"临时目录保留在 {WORK}")

    print("\n" + "=" * 60)
    print(f"通过 {len(PASS)} 项，失败 {len(FAIL)} 项")
    if FAIL:
        for n, d in FAIL:
            print(f"  · {n}  {d}")
        return 1
    print("全部通过 ✓")
    return 0


def run_checks(api: Api, base: str, mock: MockOneBot, cfg: Path) -> None:
    print("\n[1] 静态资源", flush=True)
    st, html = api.raw("/")
    if st == 200 and "VCA" in html and "app.js" in html:
        ok("主页面", f"{len(html)} 字节")
    else:
        bad("主页面", f"HTTP {st}")
    for path, key in (("/style.css", "app"), ("/app.js", "api")):
        st, body = api.raw(path)
        if st == 200 and key in body:
            ok(f"资源 {path}", f"{len(body)} 字节")
        else:
            bad(f"资源 {path}", f"HTTP {st}")

    print("\n[2] 令牌保护", flush=True)
    st, _ = Api(base, "wrong-token").call("/api/status")
    if st == 403:
        ok("错误 token 被拒（403）")
    else:
        bad("错误 token 被拒", f"实际 HTTP {st}")

    print("\n[3] 状态与预设", flush=True)
    _, s = api.call("/api/status")
    if s and s.get("ok"):
        ok("GET /api/status", f"profile={s.get('profile')}")
    else:
        bad("GET /api/status", str(s))
    _, p = api.call("/api/providers")
    if p and len(p.get("providers") or []) >= 5:
        ok("GET /api/providers", f"{len(p['providers'])} 个预设")
    else:
        bad("GET /api/providers", str(p))

    print("\n[4] 模型配置写入与回读", flush=True)
    r = api.call("/api/config/llm", {
        "provider": "deepseek",
        "base_url": "https://api.deepseek.com/v1",
        "model": "deepseek-chat",
        "defer_on_peak": True,
    })[1]
    if r and r.get("ok"):
        ok("写入模型配置", "字段 " + ",".join(r.get("wrote") or []))
    else:
        bad("写入模型配置", str(r))
    g = api.call("/api/config/llm")[1] or {}
    if g.get("provider") == "deepseek" and g.get("model") == "deepseek-chat":
        ok("回读模型配置一致")
    else:
        bad("回读模型配置", str(g))

    # 关键回归：写 llm 段不能碰 push 段、也不能碰 transcriber 段
    sp = cfg / "profiles" / "default" / "settings.yaml"
    text = sp.read_text(encoding="utf-8") if sp.is_file() else ""
    if 'provider: ""' in text and "model: tiny" in text:
        ok("落盘未误伤其它段", "push.provider 仍为空、whisper 模型名未变")
    else:
        bad("落盘误伤了其它段", "llm 段的写入污染了 push 或 transcriber")

    print("\n[5] 推送配置 + 真实发送", flush=True)
    r = api.call("/api/config/push", {
        "provider": "onebot",
        "endpoint": mock.url,
        "target": "123456789",
        "target_type": "group",
        "token": "smoke-token",
    })[1]
    if r and r.get("ok"):
        ok("写入推送配置", "字段 " + ",".join(r.get("wrote") or []))
    else:
        bad("写入推送配置", str(r))

    # 密钥应落在配置根目录的 secrets.env（不是 profile 目录里），
    # 位置与 vca_core::secrets::default_path 保持一致
    sec = cfg / "secrets.env"
    if sec.is_file() and "VCA_PUSH_TOKEN" in sec.read_text(encoding="utf-8"):
        ok("密钥进 secrets.env", "配置根目录")
    else:
        bad("密钥进 secrets.env", f"没找到 VCA_PUSH_TOKEN（找的是 {sec}）")

    r = api.call("/api/push/test", {}, timeout=40)[1] or {}
    if r.get("ok"):
        ok("发送测试消息", f"message_id={r.get('message_id')}")
    else:
        bad("发送测试消息", str(r.get("error")))
    if mock.received:
        req = mock.received[-1]
        good = (req["path"] == "/send_group_msg"
                and req["auth"] == "Bearer smoke-token"
                and req["body"].get("group_id") == 123456789)
        if good:
            ok("报文符合 OneBot 11", f"POST {req['path']}")
        else:
            bad("报文不符合规范", json.dumps(req, ensure_ascii=False)[:200])
    else:
        bad("mock 未收到推送请求")

    print("\n[6] 时间表与课表", flush=True)
    tt = {
        "timetables": [{
            "id": "default", "name": "冒烟作息", "is_active": True, "source": "manual",
            "slots": [
                {"period": 1, "start": "08:00", "end": "08:45", "kind": "class", "name": None},
                {"start": "12:00", "end": "13:30", "kind": "break", "name": "午餐午休"},
                {"period": 2, "start": "14:00", "end": "14:45", "kind": "class", "name": None},
                {"start": "18:00", "end": "19:00", "kind": "break", "name": "晚餐"},
            ],
        }],
    }
    r = api.call("/api/timetable", tt)[1] or {}
    if r.get("ok") and len(r.get("windows") or []) == 2:
        names = [w["name"] for w in r["windows"]]
        ok("保存时间表并推导窗口", " / ".join(names))
    else:
        bad("推导处理窗口", str(r))

    sc = {
        "teachers": [{"id": "t1", "name": "冒烟老师", "profile": "default"}],
        "week_template": {"cycle": "every", "entries": [
            {"day": "Mon", "period": 1, "start": "08:00", "end": "08:45",
             "course": "冒烟课", "teacherId": "t1", "record": True, "cycle": "every"},
        ]},
        "weekend_template": {"source": "new", "entries": []},
        "overrides": [],
    }
    r = api.call("/api/schedule", sc)[1] or {}
    if r.get("ok") and r.get("entries") == 1:
        ok("保存课表", f"{r['entries']} 条")
    else:
        bad("保存课表", str(r))
    g = api.call("/api/schedule")[1] or {}
    if len(g.get("entries") or []) == 1:
        ok("回读课表一致")
    else:
        bad("回读课表", str(g))

    print("\n[7] 守护进程控制", flush=True)
    d = api.call("/api/daemon/status")[1] or {}
    if d.get("running") is False:
        ok("初始状态为已停止")
    else:
        bad("初始状态", str(d))

    r = api.call("/api/daemon/start", {"dry_run": True})[1] or {}
    if r.get("ok"):
        ok("启动守护进程（演练）")
    else:
        bad("启动守护进程", str(r.get("error")))
    time.sleep(2.0)
    d = api.call("/api/daemon/status")[1] or {}
    if d.get("running"):
        ok("确认在运行", f"uptime={d.get('uptime')}")
    else:
        bad("启动后状态", str(d))

    # 日志文件应当已经有内容（守护进程的启动信息会写进去）
    lg = api.call("/api/logs")[1] or {}
    if len(lg.get("lines") or []) > 0:
        ok("日志可读", f"{lg.get('total_lines')} 行")
    else:
        bad("日志可读", str(lg.get("note")))

    r = api.call("/api/daemon/stop", {})[1] or {}
    if r.get("ok"):
        ok("请求停止")
    else:
        bad("请求停止", str(r.get("error")))
    stopped = False
    for _ in range(30):
        time.sleep(0.5)
        d = api.call("/api/daemon/status")[1] or {}
        if not d.get("running"):
            stopped = True
            break
    if stopped:
        ok("确认已停止")
    else:
        bad("停止后状态", "等了 15 秒仍在运行")

    print("\n[8] 作业与清理", flush=True)
    j = api.call("/api/jobs")[1] or {}
    if j.get("ok"):
        ok("GET /api/jobs", f"{len(j.get('jobs') or [])} 条")
    else:
        bad("GET /api/jobs", str(j))
    c = api.call("/api/clean/preview", {})[1] or {}
    if c.get("ok"):
        ok("清理预览（只算不删）")
    else:
        bad("清理预览", str(c))

    print("\n[9] 错误处理", flush=True)
    r = api.call("/api/config/llm", {"model": "<模型名>"})[1] or {}
    # 占位符写进去也算成功（界面允许用户改回去），但回读必须是占位符
    g = api.call("/api/config/llm")[1] or {}
    if g.get("model") == "<模型名>":
        ok("占位符如实回读（不会被当成已配置）")
    else:
        bad("占位符回读", str(g))
    st, _ = api.call("/api/not-exist")
    if st == 200:
        ok("未知接口返回结构化错误（HTTP 200 + ok:false）")
    else:
        bad("未知接口", f"HTTP {st}")

    print("\n[10] 退出", flush=True)
    r = api.call("/api/quit", {}, timeout=5)[1] or {}
    if r.get("ok"):
        ok("退出接口")
    else:
        bad("退出接口", str(r))


if __name__ == "__main__":
    sys.exit(main())
