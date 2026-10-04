#!/usr/bin/env python3
"""Make RetroGit's icons from the full logo (1254x1254 PNG): needs Pillow.

    python3 scripts/make-icons.py path/to/retrogit.png

Writes raw RGBA files (no image decoder needed at run time), RetroGit.ico (Windows),
RetroGit.icns (macOS, needs iconutil) and the README logo.
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
# Windows: the notifications' icon (written next to the app data at start).
icon.resize((256, 256), Image.LANCZOS).save(assets / "icon-256.png", optimize=True)
# Windows: one .ico with the usual sizes (the .exe icon and the installer).
icon.save(assets / "RetroGit.ico", sizes=[(s, s) for s in (16, 24, 32, 48, 64, 128, 256)])
# macOS: RetroGit.icns through Apple's iconutil (skipped elsewhere).
import shutil
import subprocess
import tempfile

if shutil.which("iconutil"):
    with tempfile.TemporaryDirectory() as tmp:
        iconset = Path(tmp) / "RetroGit.iconset"
        iconset.mkdir()
        for size in (16, 32, 128, 256, 512):
            icon.resize((size, size), Image.LANCZOS).save(iconset / f"icon_{size}x{size}.png")
            big = icon.resize((size * 2, size * 2), Image.LANCZOS)
            big.save(iconset / f"icon_{size}x{size}@2x.png")
        subprocess.run(
            ["iconutil", "-c", "icns", str(iconset), "-o", str(assets / "RetroGit.icns")],
            check=True,
        )
