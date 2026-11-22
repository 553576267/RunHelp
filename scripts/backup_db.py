#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""RunHelp 数据库安全备份工具。

为什么不能直接 copy runhelp.db：
  SQLite 运行时可能有未 checkpoint 的事务留在 -wal 文件里，直接复制主库文件
  会漏掉这部分数据，得到一个"看起来能打开但少数据"的库。本脚本用 SQLite 官方
  的在线备份 API（Connection.backup），无论源库是否正在写入，都能拿到一致性快照。

用法（可作为 RunHelp 命令运行，Shell 选 PowerShell/CMD，超时 0）：
  python scripts\\backup_db.py
  python scripts\\backup_db.py --dir "D:\\坚果云\\RunHelp备份"
  python scripts\\backup_db.py --db "C:\\...\\runhelp.db" --dir ... --keep 20

参数：
  --db    源数据库路径；不传则自动解析（优先读指针文件，再回退默认路径）
  --dir   备份目录；不传则读环境变量 RUNHELP_BACKUP_DIR，再回退坚果云默认目录
  --keep  保留最近 N 份本机备份，超出的自动删除（默认 12）；其它电脑前缀的文件不动
"""
import argparse
import json
import os
import re
import socket
import sqlite3
import sys
import time
from datetime import datetime
from pathlib import Path

APP_ID = "com.runhelp.desktop"


def default_appdata() -> Path:
    appdata = os.environ.get("APPDATA")
    if appdata:
        return Path(appdata) / APP_ID
    return Path.home() / "AppData" / "Roaming" / APP_ID


def resolve_source_db(explicit: str | None) -> Path:
    """解析源数据库路径：显式 > 指针文件 > 默认路径。"""
    if explicit:
        p = Path(explicit).expanduser()
        if not p.is_file():
            raise FileNotFoundError(f"指定的数据库不存在: {p}")
        return p

    base = default_appdata()
    pointer = base / "db_location.json"
    if pointer.is_file():
        try:
            data = json.loads(pointer.read_text(encoding="utf-8"))
            p = Path(data.get("path", ""))
            if p.is_absolute() and p.is_file():
                return p
        except Exception:
            pass  # 指针损坏则回退默认路径
    fallback = base / "runhelp.db"
    if not fallback.is_file():
        raise FileNotFoundError(f"找不到数据库: {fallback}（可用 --db 指定）")
    return fallback


def resolve_backup_dir(explicit: str | None, src: Path | None = None) -> Path:
    """备份目录：显式 > 数据库 settings.backup_dir > 环境变量 > 坚果云默认目录。

    从 settings 表读是为了让"脚本备份"和"软件里恢复"始终指向同一个目录：
    用户在软件设置页填一次即可，命令不用再带 --dir。
    """
    raw = explicit
    if not raw and src:
        try:
            c = sqlite3.connect(f"file:{src}?mode=ro", uri=True)
            row = c.execute("SELECT value FROM settings WHERE key='backup_dir'").fetchone()
            c.close()
            if row and row[0]:
                raw = row[0]
        except Exception:
            pass
    raw = raw or os.environ.get("RUNHELP_BACKUP_DIR")
    if not raw:
        # 坚果云默认同步目录；两台电脑各自用自己用户名下的这份
        raw = str(Path.home() / "Nutstore" / "RunHelpBackups")
    return Path(raw).expanduser()


def safe_hostname() -> str:
    """机器名做文件名前缀，去掉空格等不友好字符。"""
    name = socket.gethostname() or "pc"
    return re.sub(r"[^A-Za-z0-9_-]", "_", name).strip("_") or "pc"


def backup(src: Path, dst_dir: Path, keep: int) -> Path:
    dst_dir.mkdir(parents=True, exist_ok=True)
    stamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    dst = dst_dir / f"runhelp_{safe_hostname()}_{stamp}.db"

    # 在线备份：源库以只读方式打开，backup API 保证一致性快照
    src_conn = sqlite3.connect(f"file:{src}?mode=ro", uri=True)
    dst_conn = sqlite3.connect(str(dst))
    try:
        src_conn.backup(dst_conn)
    finally:
        dst_conn.close()
        src_conn.close()

    # 关键：源库是 WAL 模式，备份库会继承。WAL 模式下数据可能滞留在 -wal 文件，
    # 单独拷走 .db 到另一台电脑会丢数据。切到 DELETE 模式会把 WAL 内容合并进
    # 主库并移除 -wal/-shm，使备份成为自包含的单文件。
    fix = sqlite3.connect(str(dst))
    try:
        fix.execute("PRAGMA journal_mode=DELETE")
        fix.commit()
    finally:
        fix.close()

    # 完整性校验：integrity_check 通过 + 能查出项目数，才是有效备份
    verify = sqlite3.connect(f"file:{dst}?mode=ro", uri=True)
    try:
        ok = verify.execute("PRAGMA integrity_check").fetchone()[0]
        if ok != "ok":
            raise RuntimeError(f"备份完整性校验失败: {ok}")
        n = verify.execute("SELECT COUNT(*) FROM projects").fetchone()[0]
    finally:
        verify.close()

    size = dst.stat().st_size
    print(f"[备份成功] {dst}")
    print(f"          项目数 {n}，大小 {size / 1024:.1f} KB")

    # 保留最近 keep 份（按修改时间）。备份目录可能是坚果云同步目录，
    # 里面同时放着其它电脑生成的备份，只能清理本机前缀的那一部分。
    local_prefix = f"runhelp_{safe_hostname()}_"
    all_files = [f for f in dst_dir.glob("runhelp_*.db") if f.is_file()]
    files = sorted(
        (f for f in all_files if f.name.startswith(local_prefix)),
        key=lambda f: f.stat().st_mtime,
        reverse=True,
    )
    other_count = len(all_files) - len(files)
    removed = 0
    for old in files[keep:]:
        try:
            old.unlink()
            removed += 1
        except OSError:
            pass
        # 顺手清理可能残留的 WAL 附属文件
        for suffix in ("-wal", "-shm"):
            sidecar = Path(str(old) + suffix)
            if sidecar.exists():
                try:
                    sidecar.unlink()
                except OSError:
                    pass
    others = f"，其它电脑 {other_count} 份未清理" if other_count else ""
    print(f"          本机备份保留 {min(len(files), keep)} 份，已清理 {removed} 份旧备份{others}")
    return dst


def main() -> int:
    parser = argparse.ArgumentParser(description="RunHelp 数据库安全备份")
    parser.add_argument("--db", help="源数据库路径（默认自动解析）")
    parser.add_argument("--dir", help="备份目录（默认坚果云 RunHelpBackups）")
    parser.add_argument("--keep", type=int, default=12, help="保留最近 N 份（默认 12）")
    args = parser.parse_args()

    try:
        src = resolve_source_db(args.db)
        dst_dir = resolve_backup_dir(args.dir, src)
        print(f"[源数据库] {src}")
        print(f"[备份目录] {dst_dir}")
        backup(src, dst_dir, args.keep)
        return 0
    except Exception as e:
        print(f"[备份失败] {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
