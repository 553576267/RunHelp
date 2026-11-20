#!/usr/bin/env python3
"""RunHelp 图片压缩脚本。

用法:
    python compress_images.py --config config.json
    python compress_images.py --config -          # 从 stdin 读取 JSON

输出: 每行一条 JSON，便于桌面端逐行解析。
  {"type":"progress","current":1,"total":10,"file":"a.png"}
  {"type":"result","ok":true,"input":"a.png","output":"out/a.jpg","before":100,"after":30}
  {"type":"error","message":"..."}
  {"type":"summary","total":10,"ok":9,"failed":1,"before":1000,"after":300}

配置字段:
  input_paths  : 字符串数组，文件或目录
  output_dir   : 输出目录，空串表示覆盖原目录(_compressed 子目录)
  quality      : 1-95，默认 80
  max_width    : 0 表示不限制
  max_height   : 0 表示不限制
  format       : keep | jpg | png | webp
  naming       : keep | prefix | suffix
  name_prefix  : 前缀文本
  name_suffix  : 后缀文本
  strip_metadata: 是否去除元数据
  recursive    : 是否递归子目录
  overwrite    : 是否覆盖同名文件
  min_size_kb  : 小于该体积的文件跳过，默认 0
"""

import argparse
import json
import os
import sys
from pathlib import Path

SUPPORTED = {".jpg", ".jpeg", ".png", ".webp", ".bmp", ".gif", ".tif", ".tiff"}


def emit(obj: dict) -> None:
    sys.stdout.write(json.dumps(obj, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def load_config(path: str) -> dict:
    if path == "-":
        return json.loads(sys.stdin.read())
    with open(path, "r", encoding="utf-8") as f:
        return json.load(f)


def collect_files(paths: list, recursive: bool) -> list:
    found = []
    for raw in paths:
        p = Path(raw)
        if p.is_file():
            if p.suffix.lower() in SUPPORTED:
                found.append(p)
        elif p.is_dir():
            iterator = p.rglob("*") if recursive else p.glob("*")
            for item in iterator:
                if item.is_file() and item.suffix.lower() in SUPPORTED:
                    found.append(item)
        else:
            emit({"type": "error", "message": f"路径不存在: {raw}"})
    return found


def resolve_output(src: Path, cfg: dict) -> Path:
    out_dir = cfg.get("output_dir", "") or str(src.parent / "_compressed")
    out_dir_path = Path(out_dir)
    fmt = cfg.get("format", "keep")
    if fmt == "keep":
        ext = src.suffix.lower()
    elif fmt == "jpg":
        ext = ".jpg"
    else:
        ext = "." + fmt

    naming = cfg.get("naming", "keep")
    stem = src.stem
    if naming == "prefix":
        stem = str(cfg.get("name_prefix", "")) + stem
    elif naming == "suffix":
        stem = stem + str(cfg.get("name_suffix", "_compressed"))

    target = out_dir_path / f"{stem}{ext}"
    if not cfg.get("overwrite", True) and target.exists():
        idx = 1
        while target.exists():
            target = out_dir_path / f"{stem}_{idx}{ext}"
            idx += 1
    return target


def compress_one(src: Path, target: Path, cfg: dict, image_module) -> tuple:
    before = src.stat().st_size
    img = image_module.open(src)
    img.load()
    if img.mode in ("RGBA", "LA", "P"):
        has_alpha = "A" in img.getbands() or img.info.get("transparency") is not None
        if not has_alpha or cfg.get("format", "keep") in ("jpg", "jpeg"):
            background = image_module.new("RGB", img.size, (255, 255, 255))
            if "A" in img.getbands():
                background.paste(img, mask=img.getchannel("A"))
            else:
                background.paste(img)
            img = background
        else:
            img = img.convert("RGBA")
    else:
        img = img.convert("RGB")

    mw = int(cfg.get("max_width", 0) or 0)
    mh = int(cfg.get("max_height", 0) or 0)
    if mw > 0 or mh > 0:
        img.thumbnail((mw or 100000, mh or 100000), image_module.LANCZOS)

    target.parent.mkdir(parents=True, exist_ok=True)
    save_kwargs = {}
    fmt = cfg.get("format", "keep")
    actual_ext = target.suffix.lower()
    if actual_ext in (".jpg", ".jpeg"):
        save_kwargs = {"quality": int(cfg.get("quality", 80)), "optimize": True, "progressive": True}
    elif actual_ext == ".webp":
        save_kwargs = {"quality": int(cfg.get("quality", 80)), "method": 6}
    elif actual_ext == ".png":
        save_kwargs = {"optimize": True}
        if cfg.get("strip_metadata", True):
            save_kwargs.pop("optimize", None)
    if cfg.get("strip_metadata", True):
        save_kwargs["exif"] = b""

    try:
        img.save(target, **save_kwargs)
    except TypeError:
        img.save(target)
    after = target.stat().st_size
    return before, after


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", required=True)
    args = parser.parse_args()

    try:
        cfg = load_config(args.config)
    except Exception as exc:
        emit({"type": "error", "message": f"读取配置失败: {exc}"})
        return 2

    try:
        from PIL import Image
    except ImportError:
        emit({"type": "error", "message": "缺少 Pillow，请先执行: pip install Pillow"})
        return 3

    files = collect_files(cfg.get("input_paths", []), bool(cfg.get("recursive", False)))
    total = len(files)
    min_bytes = int(cfg.get("min_size_kb", 0) or 0) * 1024
    ok_count = 0
    failed = 0
    sum_before = 0
    sum_after = 0

    for idx, src in enumerate(files, start=1):
        emit({"type": "progress", "current": idx, "total": total, "file": str(src)})
        try:
            if src.stat().st_size < min_bytes:
                emit({"type": "result", "ok": True, "skipped": True, "input": str(src), "output": "", "before": src.stat().st_size, "after": src.stat().st_size})
                sum_before += src.stat().st_size
                sum_after += src.stat().st_size
                ok_count += 1
                continue
            target = resolve_output(src, cfg)
            before, after = compress_one(src, target, cfg, Image)
            sum_before += before
            sum_after += after
            ok_count += 1
            emit({"type": "result", "ok": True, "input": str(src), "output": str(target), "before": before, "after": after})
        except Exception as exc:
            failed += 1
            emit({"type": "error", "message": f"{src}: {exc}"})

    emit({"type": "summary", "total": total, "ok": ok_count, "failed": failed, "before": sum_before, "after": sum_after})
    return 0 if failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
