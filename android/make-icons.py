#!/usr/bin/env python3
"""favicon.ico -> mipmap-*/ic_launcher.png (берётся самый крупный кадр). Запуск: python3 android/make-icons.py favicon.ico"""
import os, sys
from PIL import Image

src = sys.argv[1] if len(sys.argv) > 1 else "favicon.ico"
im = Image.open(src)
sizes = im.info.get("sizes") or getattr(im.ico, "sizes", lambda: [im.size])()
im.size = max(sizes)
im = im.convert("RGBA")
base = os.path.join(os.path.dirname(os.path.abspath(__file__)), "app/src/main/res")
for n, sz in [("mdpi", 48), ("hdpi", 72), ("xhdpi", 96), ("xxhdpi", 144), ("xxxhdpi", 192)]:
    d = os.path.join(base, f"mipmap-{n}")
    os.makedirs(d, exist_ok=True)
    im.resize((sz, sz), Image.LANCZOS).save(os.path.join(d, "ic_launcher.png"))
print("ok", im.size)
