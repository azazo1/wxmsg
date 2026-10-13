#!/usr/bin/env python3
"""扫描仓库是否混入个人信息或密钥.

默认扫描 git 未忽略的全部文件 (含尚未提交的新文件), 逐行匹配敏感模式, 命中时打印
`文件:行号: 说明` 并以非零码退出, 便于直接挂进提交前的检查.

项目专属的敏感词 (别名, 备注名等) 写在 `secrets/private-terms.txt`, 每行一个, 以 `#`
开头的行忽略. 该文件已被 `.gitignore` 排除, 因此敏感词本身不会随脚本一起入库.

用法:
    python3 scripts/check-private.py             # 扫描工作区
    python3 scripts/check-private.py --history   # 扫描全部 git 历史里的补丁内容
    python3 scripts/check-private.py --list      # 只列出将被扫描的文件
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

# 允许出现的值, 命中后不再报告:
# - 文档里作为示例的假标识;
# - 微信主二进制与 arm64 切片的 SHA-256. 公开应用的构建摘要不是隐私, 文档靠它确认
#   分析的是同一个版本.
ALLOWED_VALUES = {
    "wxid_a1b2c3d4e5f60000",
    "764966cdaaf945bc8b23968bb7b3dca3cdc4067e2891a38e28c7556788e0682c",
    "f7f9b8044c911e674d5a7c8b377f410a2784c07d67d80b5bce188ed221b199be",
}

# 密钥模式的说明文字, 也用于按文件豁免.
HEX_LABEL = "疑似密钥 (64 位十六进制)"

# 这些文件里的 64 位十六进制是依赖校验和, 不是密钥.
HEX_EXEMPT_FILES = {"Cargo.lock"}

# 通用模式, 与本机具体内容无关. 只查 macOS 的家目录形式 `/Users/<名字>`;
# 文档里的 `<tmp>/home/Library/...` 是占位路径, 不应当作泄露.
BASE_PATTERNS: list[tuple[str, str]] = [
    ("疑似真实 wxid", r"wxid_[a-z0-9]{11,}"),
    (HEX_LABEL, r"\b[0-9a-fA-F]{64}\b"),
    ("本机家目录路径", r"/Users/[A-Za-z0-9._-]+"),
    ("疑似群或开放平台 id", r"\b\d{9,}@(?:chatroom|openim)\b"),
]


def repo_root() -> Path:
    """仓库根目录."""
    result = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        capture_output=True,
        text=True,
        check=True,
    )
    return Path(result.stdout.strip())


def workspace_files(root: Path) -> list[Path]:
    """git 未忽略的文件, 包含尚未提交的新文件."""
    result = subprocess.run(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        cwd=root,
        capture_output=True,
        check=True,
    )
    return [root / name.decode() for name in result.stdout.split(b"\0") if name]


def load_patterns(root: Path) -> list[tuple[str, re.Pattern[str]]]:
    """通用模式加上 `secrets/private-terms.txt` 里的专属敏感词."""
    patterns = [(label, re.compile(pattern)) for label, pattern in BASE_PATTERNS]
    terms = root / "secrets" / "private-terms.txt"
    if terms.exists():
        for line in terms.read_text(encoding="utf-8").splitlines():
            term = line.strip()
            if term and not term.startswith("#"):
                patterns.append((f"专属敏感词 {term!r}", re.compile(re.escape(term))))
    return patterns


def is_binary(path: Path) -> bool:
    """按前 8 KB 是否含 NUL 字节判断, 避免把二进制当文本行处理."""
    try:
        with path.open("rb") as handle:
            return b"\0" in handle.read(8192)
    except OSError:
        return True


def scan_lines(text: str, patterns, report) -> int:
    """逐行扫描, 返回命中条数."""
    hits = 0
    for number, line in enumerate(text.splitlines(), start=1):
        for label, pattern in patterns:
            for match in pattern.finditer(line):
                if match.group(0) in ALLOWED_VALUES:
                    continue
                report(number, label, match.group(0))
                hits += 1
    return hits


def patterns_for(path: Path, patterns) -> list[tuple[str, re.Pattern[str]]]:
    """按文件名裁掉不适用的模式."""
    if path.name in HEX_EXEMPT_FILES:
        return [(label, pattern) for label, pattern in patterns if label != HEX_LABEL]
    return patterns


def scan_workspace(root: Path, patterns) -> int:
    """扫描工作区文件."""
    total = 0
    for path in workspace_files(root):
        if not path.is_file() or is_binary(path):
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        name = path.relative_to(root)

        def report(number, label, hit, name=name):
            nonlocal total
            total += 1
            print(f"{name}:{number}: {label}: {hit}")

        scan_lines(text, patterns_for(path, patterns), report)
    return total


def scan_history(root: Path, patterns) -> int:
    """扫描全部历史提交的补丁, 用于确认没有敏感内容进过历史."""
    result = subprocess.run(
        ["git", "log", "--all", "--patch", "--no-color", "--unified=0"],
        cwd=root,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        # 还没有任何提交时 git log 会报错, 这不算问题.
        print("历史为空, 无需扫描")
        return 0
    total = 0

    def report(number, label, hit):
        nonlocal total
        total += 1
        print(f"history:{number}: {label}: {hit}")

    scan_lines(result.stdout, patterns, report)
    return total


def main() -> int:
    parser = argparse.ArgumentParser(description="扫描仓库是否混入个人信息或密钥")
    parser.add_argument("--history", action="store_true", help="改扫 git 历史里的补丁内容")
    parser.add_argument("--list", action="store_true", help="只列出将被扫描的文件")
    args = parser.parse_args()

    root = repo_root()
    if args.list:
        for path in workspace_files(root):
            print(path.relative_to(root))
        return 0

    patterns = load_patterns(root)
    hits = scan_history(root, patterns) if args.history else scan_workspace(root, patterns)
    if hits:
        print(f"\n发现 {hits} 处可疑内容, 请先处理再提交")
        return 1
    print("未发现可疑内容")
    return 0


if __name__ == "__main__":
    sys.exit(main())
