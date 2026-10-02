#!/usr/bin/env python3
"""Make RetroGit's icons from the full logo (1254x1254 PNG): needs Pillow.

    python3 scripts/make-icons.py path/to/retrogit.png

Writes raw RGBA files (no image decoder needed at run time) and the README logo.
"""
import sys
from pathlib import Path

from PIL import Image

root = Path(__file__).resolve().parent.parent
logo = Image.open(sys.argv[1]).convert("RGBA")
w = logo.width
# The window and the Git diamond, without the "RetroGit" button below them.
icon = logo.crop((round(w * 0.199), round(w * 0.080), round(w * 0.801), round(w * 0.682)))
assets = root / "crates" / "app" / "assets"
for size in (32, 256):
    (assets / f"icon-{size}.rgba").write_bytes(icon.resize((size, size), Image.LANCZOS).tobytes())
(assets / "logo-128.rgba").write_bytes(logo.resize((128, 128), Image.LANCZOS).tobytes())
logo.resize((256, 256), Image.LANCZOS).save(root / "docs" / "logo.png", optimize=True)
