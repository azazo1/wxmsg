#!/usr/bin/env python3
"""密钥抓取 sidecar.

职责很窄: 启动 (或附加到) 微信副本, 载入 `hook.js`, 把钩子报上来的 32 字节密钥收集起来写成
JSON。**不判断哪把密钥属于哪个库** —— 那一步交给主程序用 HMAC 校验做, 因为它要读库文件。

用法:

    uv run --project sidecar sidecar/capture.py \\
        --target /path/to/WeChat.app/Contents/MacOS/WeChat \\
        --hook sidecar/hook.js --home /tmp/副本家目录 --out keys.json --timeout 600
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import sys
import time

# 收到 Ctrl-C 时不要把已经抓到的密钥丢掉。
stopping = False


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="收集微信进程里的 32 字节密钥")
    parser.add_argument("--target", help="要启动的可执行文件; 与 --pid 二选一")
    parser.add_argument(
        "--arg",
        action="append",
        default=[],
        dest="program_args",
        metavar="VALUE",
        help="传给被启动程序的参数, 可重复; 值以横杠开头时写成 --arg=-x",
    )
    parser.add_argument("--pid", type=int, help="附加到已经在跑的进程")
    parser.add_argument("--hook", required=True, help="hook.js 的路径")
    parser.add_argument("--out", required=True, help="收集结果写到哪里")
    parser.add_argument("--home", help="启动时把 HOME 指向这个目录")
    parser.add_argument("--timeout", type=float, default=600.0, help="最长收集多少秒")
    parser.add_argument("--quiet", action="store_true", help="只输出密钥, 不输出其它上报")
    parser.add_argument(
        "--keep-running",
        action="store_true",
        help="结束时不要关掉副本 (默认会关掉)",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if not args.target and not args.pid:
        print("必须给出 --target 或 --pid", file=sys.stderr)
        return 2

    try:
        import frida
    except ImportError:
        print("没有找到 frida; 请用 uv 运行本脚本, 例如", file=sys.stderr)
        print("  uv run --project sidecar sidecar/capture.py ...", file=sys.stderr)
        return 3

    with open(args.hook, "r", encoding="utf-8") as handle:
        source = handle.read()

    # hex -> 出现过的上报标签
    keys: dict[str, set[str]] = {}
    messages: list[str] = []
    started = time.time()

    def on_message(message: dict, _data: bytes | None) -> None:
        nonlocal keys, messages
        if message.get("type") == "send":
            payload = str(message.get("payload", ""))
            if payload.startswith("KEY32 "):
                parts = payload.split(" ", 2)
                if len(parts) == 3:
                    _, tag, hexkey = parts
                    keys.setdefault(hexkey, set()).add(tag)
                    print(f"[密钥] {tag} {hexkey}", flush=True)
                    return
            messages.append(payload)
            if not args.quiet:
                print(f"[上报] {payload}", flush=True)
        elif message.get("type") == "error":
            detail = message.get("stack") or message.get("description") or message
            print(f"[错误] {detail}", flush=True)

    def on_stop(_signum, _frame) -> None:
        global stopping
        stopping = True

    signal.signal(signal.SIGINT, on_stop)
    signal.signal(signal.SIGTERM, on_stop)

    device = frida.get_local_device()
    pid: int | None = None
    session = None

    return_code = 0
    try:
        try:
            if args.pid:
                print(f"[启动] 附加到进程 {args.pid}", flush=True)
                session = device.attach(args.pid)
                pid = args.pid
            else:
                env = dict(os.environ)
                if args.home:
                    # 微信按 家目录 + Library/Containers/<bundle> 拼数据根, 因此改这两个就够。
                    env["HOME"] = args.home
                    env["CFFIXED_USER_HOME"] = args.home
                    print(f"[启动] 启动副本, 家目录 {args.home}", flush=True)
                else:
                    print("[启动] 启动副本, 沿用当前家目录", flush=True)
                pid = device.spawn([os.path.abspath(args.target), *args.program_args], env=env)

            if session is None:
                session = device.attach(pid)

            script = session.create_script(source)
            script.on("message", on_message)
            script.load()
            if args.pid is None:
                device.resume(pid)
                print(f"[启动] 已恢复进程 {pid}, 现在可以扫码登录", flush=True)
        except frida.PermissionDeniedError as error:
            # 在受限环境 (沙箱, 未授权的终端) 里 macOS 不允许取得 task_for_pid。
            print(f"[错误] 没有权限访问目标进程: {error}", file=sys.stderr, flush=True)
            print(
                "[错误] 请在一个被允许调试进程的终端里运行, "
                "必要时在 系统设置 -> 隐私与安全性 -> 开发者工具 里授权",
                file=sys.stderr,
                flush=True,
            )
            return_code = 4
        else:
            deadline = started + args.timeout
            while not stopping and time.time() < deadline:
                time.sleep(0.5)
                try:
                    # 进程没了就没必要再等。
                    device.get_process(pid)
                except Exception:
                    print("[启动] 目标进程已退出", flush=True)
                    break
    finally:
        payload = {
            "captured_at": int(time.time()),
            "target": args.target,
            "pid": pid,
            "seconds": round(time.time() - started, 1),
            "keys": [
                {"hex": hexkey, "tags": sorted(tags)} for hexkey, tags in sorted(keys.items())
            ],
            "messages": messages,
        }
        with open(args.out, "w", encoding="utf-8") as handle:
            json.dump(payload, handle, ensure_ascii=False, indent=2)
        print(f"[结束] 收集到 {len(keys)} 个候选密钥, 已写入 {args.out}", flush=True)
        try:
            if session is not None:
                session.detach()
        except Exception:
            pass
        # 副本是我们启动的, 就要由我们收尾; 否则它会一直占着家目录和终端。
        if args.pid is None and pid is not None and not args.keep_running:
            try:
                device.kill(pid)
                print(f"[结束] 已结束副本进程 {pid}", flush=True)
            except Exception as error:
                print(f"[结束] 结束副本进程失败, 请手动关闭: {error}", flush=True)

    return return_code


if __name__ == "__main__":
    sys.exit(main())
