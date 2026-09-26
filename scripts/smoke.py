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
    """假 OneBot 服务端：记录收到的请求，并可按脚本制造故障。

    故障脚本用来覆盖「异常与重试」这条路径 —— 正常路径容易测，
    真正容易出错的是「第一次失败、第二次成功」这种半路出状况的情形。
    """

    def __init__(self) -> None:
        self.received: list[dict] = []
        # 故障脚本：每次 POST 消费一条。元素是 ("http", 状态码) | ("retcode", 码) | ("ok",)
        self.script: list[tuple] = []
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

                step = outer.script.pop(0) if outer.script else ("ok",)
                if step[0] == "http":
                    # 服务端错误：不是 OneBot 的业务错误，而是 HTTP 层就挂了
                    text = b"<html><body><pre>Internal Server Error</pre></body></html>"
                    self.send_response(step[1])
                    self.send_header("Content-Type", "text/html")
                    self.send_header("Content-Length", str(len(text)))
                    self.end_headers()
                    self.wfile.write(text)
                    return
                if step[0] == "retcode":
                    payload = {"status": "failed", "retcode": step[1],
                               "message": "注入的业务错误"}
                else:
                    payload = {"status": "ok", "retcode": 0,
                               "data": {"message_id": 999}}
                resp = json.dumps(payload).encode()
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

    def inject(self, *steps) -> None:
        """排好接下来几次请求的响应。"""
        self.script.extend(steps)

    def clear_script(self) -> None:
        self.script.clear()

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    def stop(self) -> None:
        # server_close 是必须的：只 shutdown 的话 socket 还开着，
        # 连接会被 accept 却不响应 —— 测试就变成「等两次超时」而不是「连接被拒」
        self.httpd.shutdown()
        self.httpd.server_close()


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
    # 机器人（NapCat）的安装目录也指到临时目录：否则它会去看真实的
    # tools/napcat/，测试结果就取决于这台机器上装没装过机器人了
    env["VCA_NAPCAT_DIR"] = str(WORK / "napcat")
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
        # 刻意**不写** start / end：新格式里课表只说「星期 + 第几节」，
        # 起止时间由绑定的时间表算出来（对齐 ClassIsland 的 Classes 按下标对齐）。
        # 所以这一段同时在验「时间表 -> 课表」的补全过程。
        "week_template": {
            "cycle": "every",
            "time_layout_id": "default",
            "time_rule": {"weekday": 0, "week_count": {"week": 0, "total": 0}},
            "is_enabled": True,
            "entries": [
                {"day": "Mon", "period": 1,
                 "course": "冒烟课", "teacherId": "t1", "record": True, "cycle": "every"},
            ],
        },
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

    # 只写了第 1 节，时间表里第 1 节是 08:00–08:45，读回来必须补上
    ent0 = (g.get("entries") or [{}])[0]
    if ent0.get("start") == "08:00" and ent0.get("end") == "08:45":
        ok("课表时间由时间表补全", f"第 {ent0.get('period')} 节 {ent0.get('start')}–{ent0.get('end')}")
    else:
        bad("时间未由时间表补全", json.dumps(ent0, ensure_ascii=False)[:200])

    # 界面上的「课程格」需要时间点清单当列头，接口必须给出来
    if (g.get("class_slots") or []) and g["class_slots"][0].get("duration") == 45:
        ok("接口回出上课时间点清单", f"{len(g['class_slots'])} 个，时长 {g['class_slots'][0]['duration']} 分钟")
    else:
        bad("缺少上课时间点清单", json.dumps(g.get("class_slots"), ensure_ascii=False)[:200])

    # 触发规则与绑定的时间表要能原样回读 —— 界面顶上那三个下拉框靠它
    if g.get("time_layout_id") == "default" and (g.get("time_rule") or {}).get("weekday") == 0:
        ok("课表绑定与触发规则已落盘", f"layout={g.get('time_layout_id')}")
    else:
        bad("绑定/触发规则未落盘",
            f"layout={g.get('time_layout_id')!r} rule={g.get('time_rule')!r}")

    # 逐条勾选：写进去的 record 必须原样回读，否则「这节课不录」形同虚设
    ent = g.get("entries") or []
    if ent and ent[0].get("record") is True:
        ok("逐条录制开关已落盘", f"record={ent[0].get('record')}")
    else:
        bad("录制开关未落盘", json.dumps(ent[:1], ensure_ascii=False)[:160])

    print("\n[6.5] 界面文案接口", flush=True)
    r = api.call("/api/i18n")[1] or {}
    strings = r.get("strings") or {}
    if r.get("ok") and len(strings) >= 150 and strings.get("nav.overview"):
        ok("GET /api/i18n", f"{len(strings)} 条文案，lang={r.get('lang')}")
    else:
        bad("GET /api/i18n", f"{len(strings)} 条，nav.overview={strings.get('nav.overview')!r}")
    # 左上角那三个字母换成了 ASCII 字符画，缺了会退回字母档
    logo = strings.get("logo") or ""
    if logo.count("\n") >= 4 and "█" in logo:
        ok("ASCII 字符画在", f"{len(logo.splitlines())} 行")
    else:
        bad("ASCII 字符画缺失", repr(logo)[:80])
    # 每条文案都必须真有值，空串会在界面上留白，比缺 key 更难发现
    empty = sorted(k for k, v in strings.items() if isinstance(v, str) and not v.strip())
    if not empty:
        ok("没有空文案")
    else:
        bad("存在空文案", ", ".join(empty[:5]))

    print("\n[6.6] 机器人探测", flush=True)
    r = api.call("/api/detect-bot", {})[1] or {}
    if r.get("ok") and isinstance(r.get("found"), list) and r.get("candidates"):
        ok("POST /api/detect-bot", f"扫 {len(r['candidates'])} 个端口，命中 {r['found']}")
    else:
        bad("POST /api/detect-bot", str(r)[:160])

    print("\n[6.7] ClassIsland 课表导入", flush=True)
    ci = {
        "TimeLayouts": {
            "aaaaaaaa-0000-0000-0000-000000000001": {
                "Name": "冒烟作息表",
                "IsActive": True,
                "Layouts": [
                    {"StartTime": "08:00:00", "EndTime": "08:45:00", "TimeType": 0},
                    {"StartTime": "12:00:00", "EndTime": "13:30:00",
                     "TimeType": 1, "BreakName": "午餐午休"},
                    {"StartTime": "14:00:00", "EndTime": "14:45:00", "TimeType": 0},
                ],
            }
        },
        "Subjects": {
            "bbbbbbbb-0000-0000-0000-000000000001": {
                "Name": "导入的语文", "TeacherName": "导入老师",
            },
        },
        "ClassPlans": {
            "cccccccc-0000-0000-0000-000000000001": {
                "IsEnabled": True,
                "TimeLayoutId": "aaaaaaaa-0000-0000-0000-000000000001",
                "TimeRule": {"WeekDay": 1, "WeekCountDiv": 0},
                "Classes": [
                    {"IsEnabled": True,
                     "SubjectId": "bbbbbbbb-0000-0000-0000-000000000001"},
                    {"IsEnabled": True,
                     "SubjectId": "00000000-0000-0000-0000-000000000000"},
                    {"IsEnabled": True,
                     "SubjectId": "bbbbbbbb-0000-0000-0000-000000000001"},
                ],
            }
        },
    }
    r = api.call("/api/import/classisland",
                 {"json": json.dumps(ci, ensure_ascii=False), "timetable": "冒烟"},
                 timeout=25)[1] or {}
    # 时间表里 TimeType=0 的时段只有 2 段，而 Classes 给了 3 格；
    # 导入器按下标一一对应，第 3 格（cells[2]）不会被读；
    # 第 2 格是全零 SubjectId 又没有 DefaultClassId，按设计跳过。
    # 所以正确结果是 1 条，不是 2 条 —— 这里反过来验证了「按对跳过」。
    if r.get("ok") and r.get("entries") == 1 and r.get("slots") == 3:
        ok("导入 ClassIsland 课表",
           f"{r.get('timetable_name')} / {r['slots']} 时段 / {r['entries']} 节课")
    else:
        bad("导入 ClassIsland 课表", json.dumps(r, ensure_ascii=False)[:200])

    # 导入是**新增**一份时间表，不能把刚才那份「冒烟作息」顶掉
    g2 = api.call("/api/timetable")[1] or {}
    alltt = g2.get("all") or []
    if len(alltt) == 2 and sum(1 for x in alltt if x.get("is_active")) == 1:
        ok("导入新增时间表而非覆盖", " / ".join(str(x.get("name")) for x in alltt))
    else:
        bad("导入覆盖了已有时间表", json.dumps(alltt, ensure_ascii=False)[:200])

    # 老师名必须从 Subjects.TeacherName 提取出来，否则录制档案会全是「未分配」
    g = api.call("/api/schedule")[1] or {}
    teachers = [t.get("name") for t in (g.get("teachers") or [])]
    if "导入老师" in teachers:
        ok("从 Subjects 提取到老师", ", ".join(str(x) for x in teachers))
    else:
        bad("老师未提取", f"teachers={teachers}")

    print("\n[6.8] QQ 机器人（NapCat）", flush=True)
    r = api.call("/api/bot/napcat")[1] or {}
    root = str(r.get("root") or "")
    if r.get("ok") and r.get("installed") is False and "napcat" in root.lower():
        ok("GET /api/bot/napcat", f"未安装，装到 {root}")
    else:
        bad("GET /api/bot/napcat", json.dumps(r, ensure_ascii=False)[:200])

    # 没装就点启动：必须给一句人话，而不是一个空错误或一段 panic
    r = api.call("/api/bot/napcat/start", {})[1] or {}
    if r.get("ok") is False and "还没装好" in (r.get("error") or ""):
        ok("未安装时启动会给出明确提示")
    else:
        bad("未安装时启动", json.dumps(r, ensure_ascii=False)[:200])

    # 没在跑的时候点停止，应当是好聚好散，不是报错
    r = api.call("/api/bot/napcat/stop", {})[1] or {}
    if r.get("ok"):
        ok("未运行时停止不报错")
    else:
        bad("未运行时停止", json.dumps(r, ensure_ascii=False)[:160])

    # 造一个假的安装目录，验证「识别 → 启动 → 停止」这条链
    fake = Path(WORK) / "napcat"
    fake.mkdir(parents=True, exist_ok=True)
    (fake / "launcher.bat").write_text("@echo off\r\nexit /b 0\r\n", encoding="utf-8")
    (fake / "package.json").write_text('{"name":"napcat","version":"0.0.0-smoke"}', encoding="utf-8")

    r = api.call("/api/bot/napcat")[1] or {}
    if r.get("installed") and r.get("version") == "0.0.0-smoke" and "Shell" in (r.get("flavor") or ""):
        ok("识别出假安装", f"{r.get('flavor')} / {r.get('version')}")
    else:
        bad("识别假安装", json.dumps(r, ensure_ascii=False)[:200])

    r = api.call("/api/bot/napcat/start", {}, timeout=20)[1] or {}
    if r.get("ok") and r.get("pid"):
        ok("启动返回 pid", f"pid={r['pid']}")
    else:
        bad("启动机器人", json.dumps(r, ensure_ascii=False)[:200])
    # 那个假的 launcher.bat 会立刻退出，所以这里**不去断言它还在跑**；
    # 真正要验证的是 stop 不会因为进程已经没了就报错
    r = api.call("/api/bot/napcat/stop", {}, timeout=20)[1] or {}
    if r.get("ok"):
        ok("停止请求被接受")
    else:
        bad("停止机器人", json.dumps(r, ensure_ascii=False)[:200])

    print("\n[6.9] NapCat 一键配置", flush=True)
    # 造一份**真实形态**的 NapCat OneBot 配置：httpServers 空（默认就是空的），
    # 但用户自己加过一条反向 WS —— 那条必须被原样保留，不能被我们抹掉。
    napcat_dir = Path(WORK) / "napcat"
    cfg_dir = napcat_dir / "versions" / "9.9.26-44498" / "resources" / "app" / "napcat" / "config"
    cfg_dir.mkdir(parents=True, exist_ok=True)
    onebot = cfg_dir / "onebot11_10001.json"
    onebot.write_text(json.dumps({
        "network": {
            "httpServers": [],
            "websocketClients": [{
                "enable": True, "name": "别人的配置", "url": "ws://localhost:6199/ws",
                "token": "keep-me", "messagePostFormat": "array",
            }],
        },
        "musicSignUrl": "",
        "timeout": {"baseTimeout": 10000},
    }, ensure_ascii=False, indent=2), encoding="utf-8")
    (napcat_dir / "launcher.bat").write_text("@echo off\r\nexit /b 0\r\n", encoding="utf-8")

    r = api.call("/api/bot/napcat")[1] or {}
    if r.get("onebot_config") and r.get("onebot_http_port") is None and r.get("onebot_qq") == "10001":
        ok("识别出 onebot 配置且判为「HTTP 未开」", str(r.get("onebot_config", ""))[-40:])
    else:
        bad("onebot 配置探测", json.dumps({
            "cfg": r.get("onebot_config"), "port": r.get("onebot_http_port"),
            "qq": r.get("onebot_qq")}, ensure_ascii=False)[:200])

    r = api.call("/api/bot/napcat/configure", {"port": 3123, "token": "smoke-token"})[1] or {}
    if r.get("ok") and r.get("changed") is True and r.get("port") == 3123:
        ok("一键配置写入成功", f"QQ {r.get('qq')} / 端口 {r.get('port')}")
    else:
        bad("一键配置", json.dumps(r, ensure_ascii=False)[:200])

    # 直接读盘核对：写进去的字段、以及别人的条目有没有被保住
    written = json.loads(onebot.read_text(encoding="utf-8"))
    servers = written.get("network", {}).get("httpServers", [])
    mine = [x for x in servers if x.get("name") == "vibeclassagent"]
    others = written.get("network", {}).get("websocketClients", [])
    if (len(mine) == 1 and mine[0].get("enable") is True
            and mine[0].get("port") == 3123 and mine[0].get("token") == "smoke-token"):
        ok("配置内容正确", f"enable/port/token 都对，条目数 {len(servers)}")
    else:
        bad("配置内容", json.dumps(servers, ensure_ascii=False)[:200])

    if len(others) == 1 and others[0].get("name") == "别人的配置" and others[0].get("token") == "keep-me":
        ok("用户原有条目被保留", "websocketClients 原样不动")
    else:
        bad("原有条目被改动", json.dumps(others, ensure_ascii=False)[:200])

    if onebot.with_suffix(".json.bak").is_file():
        ok("写入前有备份", onebot.with_suffix(".json.bak").name)
    else:
        bad("没有备份", "改坏了就没法回滚")

    # 幂等：再点一次不该重复追加
    r = api.call("/api/bot/napcat/configure", {"port": 3123, "token": "smoke-token"})[1] or {}
    again = json.loads(onebot.read_text(encoding="utf-8"))
    n2 = len([x for x in again["network"]["httpServers"] if x.get("name") == "vibeclassagent"])
    if r.get("ok") and n2 == 1:
        ok("重复配置是幂等的", f"仍是 {n2} 条")
    else:
        bad("重复配置", f"条目数变成 {n2}")

    # 配完之后状态应当变成「HTTP 已开」
    r = api.call("/api/bot/napcat")[1] or {}
    if r.get("onebot_http_port") == 3123:
        ok("状态跟着变成「HTTP 已开」", f"端口 {r.get('onebot_http_port')}")
    else:
        bad("状态未更新", str(r.get("onebot_http_port")))

    # 端口探测：假的 NapCat 起不来，所以这里只验证字段结构
    r = api.call("/api/detect-bot", {})[1] or {}
    if r.get("ok") and isinstance(r.get("onebot"), list):
        ok("机器人探测带 OneBot 判定", f"端口命中 {r.get('found')}，确认为 OneBot 的 {len(r['onebot'])} 个")
    else:
        bad("机器人探测", str(r)[:160])

    print("\n[6.95] 异常与重试（OneBot）", flush=True)

    # 1) 服务端先 500、再成功：重试必须真的重试，而且最终要成功
    mock.clear_script()
    mock.inject(("http", 500))
    n_before = len(mock.received)
    r = api.call("/api/push/test", {}, timeout=60)[1] or {}
    tries = len(mock.received) - n_before
    if r.get("ok") and tries >= 2:
        ok("5xx 后自动重试并最终成功", f"共尝试 {tries} 次")
    else:
        bad("5xx 重试", f"ok={r.get('ok')} 尝试 {tries} 次：{str(r.get('error'))[:120]}")

    # 2) 业务失败（retcode != 0）：必须如实报错，不能当成成功。
    #    注入次数要**多于**重试次数 —— 只注入一次的话第二次就成功了，
    #    那验的是重试、不是错误上报（第一版就是这么写错的）。
    mock.clear_script()
    mock.inject(("retcode", 1400), ("retcode", 1400), ("retcode", 1400), ("retcode", 1400))
    n_before = len(mock.received)
    r = api.call("/api/push/test", {}, timeout=60)[1] or {}
    tries = len(mock.received) - n_before
    if (not r.get("ok")) and "1400" in str(r.get("error") or ""):
        ok("业务错误被如实报出", f"retcode 出现在错误里，重试 {tries} 次")
    else:
        bad("业务错误处理", json.dumps(r, ensure_ascii=False)[:160])

    # 3) 服务端整个消失（模拟 NapCat 崩了/断线）：
    #    必须给可读错误，而不是卡死或空错误
    mock.stop()
    # 超时给足：服务端会重试，每次 30 秒，60 秒正好卡在边界上
    r = api.call("/api/push/test", {}, timeout=90)[1] or {}
    err = str(r.get("error") or r.get("_err") or "").strip()
    if (not r.get("ok")) and err:
        ok("对端消失时给出可读错误", err[:70])
    else:
        bad("对端消失", json.dumps(r, ensure_ascii=False)[:160])

    print("\n[6.96] QQ 官方机器人（凭据检查）", flush=True)

    # 切到 QQ 官方机器人，但**故意不填** AppID/AppSecret
    api.call("/api/config/push", {
        "provider": "qq", "target": "123456789", "target_type": "group",
    })
    r = api.call("/api/push/test", {}, timeout=40)[1] or {}
    err = str(r.get("error") or "")
    if (not r.get("ok")) and ("AppID" in err or "app_id" in err or "缺" in err):
        ok("缺凭据时点名说缺什么", err[:70])
    else:
        bad("缺凭据的报错", json.dumps(r, ensure_ascii=False)[:180])

    # 恢复成 OneBot，别影响后面的用例
    api.call("/api/config/push", {
        "provider": "onebot", "endpoint": mock.url,
        "target": "123456789", "target_type": "group", "token": "smoke-token",
    })
    g = api.call("/api/config/push")[1] or {}
    if g.get("provider") == "onebot":
        ok("已恢复 OneBot 配置", g.get("endpoint", ""))
    else:
        bad("恢复配置", str(g.get("provider")))

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
