#!/usr/bin/env python3
"""Subset MiSansVF.ttf for the 78 MiB Samsung recovery partition.

The stock image's kernel (53 MB) and dtb leave ~25.8 MiB for the compressed
ramdisk; the full 20 MB variable font does not fit beside Skyward, system-init
and the stock recovery's own programs. The subset keeps every character the
UI speaks (all string literals of the source), GB2312 and Big5's common block
so file names and terminal output read whole, and the variation axis.
"""
import pathlib, subprocess, sys

SRC = '/home/yanzihan/build2/skyward/resources/fonts/MiSansVF.ttf'
OUT = '/home/yanzihan/build2/skyward/work/MiSansVF-subset.ttf'

chars = set()
# ASCII、Latin-1(® ° é)、标点、箭头、带圈数字、全角形式、符号
for rng in [(0x20, 0x7F), (0xA0, 0x100), (0x2000, 0x2070), (0x2100, 0x2150),
            (0x2190, 0x21A0), (0x2460, 0x2500), (0x2600, 0x2700),
            (0x3000, 0x3040), (0xFF00, 0xFFF0), (0x20AC, 0x20AD)]:
    chars |= {chr(c) for c in range(*rng)}
# 应用自己的全部字符串:两套中文、简繁对照和机型名都在里面
for p in pathlib.Path('/home/yanzihan/build2/skyward/crates').rglob('*.rs'):
    chars |= {c for c in p.read_text(encoding='utf-8', errors='ignore') if ord(c) > 0x7F}
# GB2312 全集(简体用户会自己敲的字)
for hi in range(0xA1, 0xF8):
    for lo in range(0xA1, 0xFF):
        try:
            chars.add(bytes([hi, lo]).decode('gb2312'))
        except Exception:
            pass
# Big5 常用区(繁体)
for hi in range(0xA4, 0xDB):
    for lo in list(range(0x40, 0x7F)) + list(range(0xA1, 0x100)):
        try:
            chars.add(bytes([hi, lo]).decode('big5'))
        except Exception:
            pass
chars.discard('\n'); chars.discard('\r'); chars.discard('\t')

unicodes = ','.join(f'U+{ord(c):04X}' for c in sorted(chars) if ord(c) >= 0x20)
pathlib.Path('/home/yanzihan/build2/skyward/work/unicodes.txt').write_text(unicodes)
print(f'字符集: {len(chars)} 个字符')

r = subprocess.run([sys.executable, '-m', 'fontTools.subset', SRC,
                    '--unicodes-file=/home/yanzihan/build2/skyward/work/unicodes.txt',
                    f'--output-file={OUT}',
                    '--layout-features=*', '--glyph-names', '--notdef-outline',
                    '--recalc-bounds', '--recalc-average-width', '--name-IDs=*'],
                   capture_output=True, text=True)
print(r.stdout[-500:] if r.stdout else '', r.stderr[-500:] if r.stderr else '')
if r.returncode == 0:
    size = pathlib.Path(OUT).stat().st_size
    print(f'子集字体: {size} 字节 ({size/1e6:.1f} MB)')
