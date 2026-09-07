#!/usr/bin/env python
"""把 studio/icons/mft-reader.png（新 logo）缩放为 256x256 并写出 studio/icons/icon.ico。

用法：python scripts/make-icon.py
"""
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "studio" / "icons" / "mft-reader.png"
DST = ROOT / "studio" / "icons" / "icon.ico"

img = Image.open(SRC).convert("RGBA").resize((256, 256), Image.LANCZOS)
img.save(DST, sizes=[(256, 256)])
print(f"icon.ico written: {DST.stat().st_size} bytes")
