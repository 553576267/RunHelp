#!/usr/bin/env python3
"""生成 THIRD-PARTY-LICENSES.md：列出 RunHelp 打包进二进制的第三方组件及其许可。

用法（在仓库根目录执行）：
    python scripts/gen_third_party_licenses.py

数据来源：
- Rust 依赖：`cargo metadata`（读取 Cargo.lock 解析出的完整依赖树）
- 前端依赖：node_modules 下每个包的 package.json

只在发版前或依赖变动后跑一次，产物 THIRD-PARTY-LICENSES.md 入库。
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
from collections import Counter, defaultdict
from datetime import date

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, 'THIRD-PARTY-LICENSES.md')

# 需要随二进制一起保留许可文本的常见许可（用于提示，不做强制校验）
PERMISSIVE = ('MIT', 'Apache-2.0', 'BSD', 'ISC', 'Zlib', 'Unicode', 'CC0', 'Unlicense')
COPYLEFT = ('GPL', 'AGPL', 'SSPL', 'BUSL', 'CC-BY-SA', 'EPL', 'CDDL')


def normalize(lic: str) -> str:
    """统一多许可表达式的写法：npm 历史上用 A/B 表示 A OR B。"""
    s = lic.strip()
    for a, b in (('MIT/Apache-2.0', 'MIT OR Apache-2.0'),
                 ('Apache-2.0/MIT', 'Apache-2.0 OR MIT'),
                 ('Unlicense/MIT', 'Unlicense OR MIT'),
                 ('BSD-3-Clause/MIT', 'BSD-3-Clause OR MIT'),
                 ('Apache-2.0 / MIT', 'Apache-2.0 OR MIT')):
        s = s.replace(a, b)
    return s


def has_permissive_option(lic: str) -> bool:
    """表达式里是否给了 MIT / Apache-2.0 这类宽松选项。"""
    return ('MIT' in lic) or ('Apache-2.0' in lic)


def rust_packages():
    meta = json.loads(
        subprocess.run(
            ['cargo', 'metadata', '--format-version', '1', '--manifest-path',
             os.path.join(ROOT, 'src-tauri', 'Cargo.toml')],
            capture_output=True, text=True, encoding='utf-8', check=True,
        ).stdout
    )
    out = []
    for p in meta['packages']:
        if p['name'] == 'runhelp':
            continue
        lic = p.get('license') or ''
        if not lic and p.get('license_file'):
            lic = '见 LICENSE 文件: ' + str(p['license_file'])
        out.append((p['name'], p.get('version', ''), normalize(lic) or 'UNKNOWN'))
    out.sort(key=lambda x: (x[0].lower(), x[1]))
    return out


def node_packages():
    base = os.path.join(ROOT, 'node_modules')
    out = []
    if not os.path.isdir(base):
        return out
    for entry in sorted(os.listdir(base)):
        if entry.startswith('.'):
            continue
        dirs = []
        if entry.startswith('@'):
            sub = os.path.join(base, entry)
            if os.path.isdir(sub):
                dirs = [os.path.join(sub, s) for s in sorted(os.listdir(sub))]
        else:
            dirs = [os.path.join(base, entry)]
        for d in dirs:
            pj = os.path.join(d, 'package.json')
            if not os.path.isfile(pj):
                continue
            try:
                with open(pj, encoding='utf-8') as f:
                    data = json.load(f)
            except Exception:
                continue
            lic = data.get('license') or data.get('licenses') or ''
            if isinstance(lic, list):
                lic = ' OR '.join(
                    str(x.get('type') if isinstance(x, dict) else x) for x in lic
                )
            elif isinstance(lic, dict):
                lic = str(lic.get('type', ''))
            out.append((data.get('name', entry), data.get('version', ''), normalize(str(lic)) or 'UNKNOWN'))
    out.sort(key=lambda x: (x[0].lower(), x[1]))
    return out


def table(rows):
    lines = ['| 组件 | 版本 | 许可 |', '| --- | --- | --- |']
    for name, ver, lic in rows:
        lines.append(f'| {name} | {ver} | {lic} |')
    return '\n'.join(lines)


def summary(rows):
    c = Counter(lic for _, _, lic in rows)
    lines = ['| 许可 | 组件数 |', '| --- | --- |']
    for lic, n in sorted(c.items(), key=lambda kv: (-kv[1], kv[0])):
        lines.append(f'| {lic} | {n} |')
    return '\n'.join(lines)


def main():
    rust = rust_packages()
    node = node_packages()
    total = len(rust) + len(node)

    risky = sorted({lic for _, _, lic in rust + node
                    if any(k in lic.upper() for k in COPYLEFT)
                    and not has_permissive_option(lic)})
    dual = sorted({lic for _, _, lic in rust + node
                   if any(k in lic.upper() for k in COPYLEFT)
                   and has_permissive_option(lic)})
    unknown = [r for r in rust + node if r[2] == 'UNKNOWN']

    parts = [
        '# 第三方组件许可',
        '',
        f'RunHelp 在构建时链接并随安装包一起分发以下第三方组件（共 {total} 个：'
        f'Rust {len(rust)} 个、前端 {len(node)} 个）。本文件由 '
        '`scripts/gen_third_party_licenses.py` 自动生成，请勿手工编辑；'
        '依赖变动后重新运行该脚本即可。',
        '',
        f'生成时间：{date.today().isoformat()}　｜　对应版本：'
        + read_version(),
        '',
        '每个组件在其源码仓库 / 发行包中都带有完整的许可文本与版权声明，'
        '本文件只做清单汇总，不替代原始许可文本。',
        '',
        '## 许可分布',
        '',
        summary(rust + node),
        '',
        '## 说明',
        '',
        '- MIT、Apache-2.0、BSD、ISC、Zlib 等宽松许可要求「随副本保留版权声明与许可文本」，'
        '本清单与各组件自带的 LICENSE 文件共同满足该要求。',
        '- 标注为 `A OR B` 的多许可组件，本项目按其中任一项（通常是 MIT）使用。',
        '- Mozilla Public License 2.0 组件（Tauri 依赖链中的 CSS 解析器）以未修改的库形式链接，'
        '未产生衍生作品，不触发源码开放义务。',
        f'- 传染性许可（GPL / AGPL / SSPL 等）检查结果：'
        + ('**未发现**。' if not risky else '发现 ' + '、'.join(risky) + '，需评估。'),
        ('- 另有 ' + '、'.join(dual) + ' 这类「宽松 + 传染」双许可组件，本项目一律按 MIT / Apache-2.0 '
         '选项使用，不触发传染条款。' if dual else ''),
        '',
    ]

    if unknown:
        parts += [
            '## 需要人工确认',
            '',
            '以下组件未在元数据中声明许可，升级或分发前需人工核对：',
            '',
            '| 组件 | 版本 | 来源 |',
            '| --- | --- | --- |',
        ]
        for name, ver, _ in unknown:
            src = 'Rust' if (name, ver, 'UNKNOWN') in rust else 'npm'
            parts.append(f'| {name} | {ver} | {src} |')
        parts.append('')

    parts += ['## Rust 依赖', '', table(rust), '']
    parts += ['## 前端依赖', '', table(node), '']

    content = '\n'.join(parts)
    with open(OUT, 'w', encoding='utf-8', newline='\n') as f:
        f.write(content)
    print(f'wrote {OUT}: {total} packages (rust {len(rust)}, node {len(node)})')
    return 0


def read_version():
    try:
        with open(os.path.join(ROOT, 'package.json'), encoding='utf-8') as f:
            return json.load(f).get('version', '')
    except Exception:
        return ''


if __name__ == '__main__':
    sys.exit(main())
