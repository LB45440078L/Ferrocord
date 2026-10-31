#!/usr/bin/env python3
"""Render the installer artwork for Windows (WiX) and macOS (DMG).

Requires Pillow (``pip install pillow``). The outputs are committed so release
builds need no image tools; run this again after changing the icon or the
copy below.

Usage::

    python3 packaging/installer-art/generate.py

Outputs:

* ``packaging/windows/wix/banner.bmp``  493x58, top banner of the WixUI dialogs
* ``packaging/windows/wix/dialog.bmp``  493x312, Welcome and Finish dialogs
* ``packaging/macos/dmg/background.png`` and ``background@2x.png``  660x400 Finder window

WixUI draws black text on top of these bitmaps: the banner keeps its left
three quarters light, and the dialog keeps everything right of x=164 white.
Finder draws icon labels in black, so the DMG background is light as well.
"""

from __future__ import annotations

from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = Path(__file__).resolve().parents[2]
ICON = ROOT / "packaging/icons/hicolor/512x512/apps/dev.ferrocord.Ferrocord.png"
FONTS = ROOT / "crates/app/assets/fonts"

ACCENT = (0xE0, 0x6B, 0x2B)
ACCENT_DEEP = (0xB4, 0x48, 0x14)
INK = (0x16, 0x17, 0x1B)
INK_SOFT = (0x2B, 0x2C, 0x33)
PAPER = (0xFF, 0xFF, 0xFF)
PAPER_WARM = (0xFB, 0xF4, 0xEE)
MUTED = (0x6B, 0x6E, 0x78)

TAGLINE = "A fast, open-source Discord client"


def font(weight: str, size: int) -> ImageFont.FreeTypeFont:
    return ImageFont.truetype(str(FONTS / f"Inter-{weight}.ttf"), size)


def lerp(a: tuple[int, ...], b: tuple[int, ...], t: float) -> tuple[int, ...]:
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def gradient(size: tuple[int, int], top: tuple[int, ...], bottom: tuple[int, ...], diagonal: bool = False) -> Image.Image:
    w, h = size
    img = Image.new("RGB", size)
    px = img.load()
    for y in range(h):
        for x in range(w):
            t = (x / max(w - 1, 1) * 0.35 + y / max(h - 1, 1) * 0.65) if diagonal else y / max(h - 1, 1)
            px[x, y] = lerp(top, bottom, t)
    return img


def icon(size: int) -> Image.Image:
    return Image.open(ICON).convert("RGBA").resize((size, size), Image.LANCZOS)


def paste_with_shadow(base: Image.Image, img: Image.Image, xy: tuple[int, int], radius: int, offset: int, alpha: int) -> None:
    shadow = Image.new("RGBA", base.size, (0, 0, 0, 0))
    mask = img.split()[3].point(lambda a: a * alpha // 255)
    shadow.paste((0, 0, 0, 255), (xy[0], xy[1] + offset), mask)
    shadow = shadow.filter(ImageFilter.GaussianBlur(radius))
    base.alpha_composite(shadow)
    base.alpha_composite(img, xy)


def centered_text(draw: ImageDraw.ImageDraw, cx: float, y: float, text: str, f: ImageFont.FreeTypeFont, fill) -> None:
    w = draw.textlength(text, font=f)
    draw.text((cx - w / 2, y), text, font=f, fill=fill)


def wix_banner(out: Path) -> None:
    w, h = 493, 58
    img = Image.new("RGBA", (w, h), PAPER + (255,))
    # A warm wash on the right, behind the logo, fading into white where WixUI writes the title.
    wash = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    px = wash.load()
    for x in range(w):
        t = max(0.0, (x - 300) / (w - 300))
        for y in range(h):
            px[x, y] = PAPER_WARM + (round(255 * t),)
    img.alpha_composite(wash)
    paste_with_shadow(img, icon(40), (w - 40 - 14, 9), radius=3, offset=1, alpha=70)
    d = ImageDraw.Draw(img)
    d.rectangle([0, h - 2, w, h], fill=ACCENT)
    img.convert("RGB").save(out, "BMP")


def wix_dialog(out: Path) -> None:
    w, h, panel = 493, 312, 164
    img = Image.new("RGBA", (w, h), PAPER + (255,))
    side = gradient((panel, h), INK_SOFT, INK).convert("RGBA")
    # Accent glow behind the logo.
    glow = Image.new("RGBA", (panel, h), (0, 0, 0, 0))
    ImageDraw.Draw(glow).ellipse([panel / 2 - 70, 40, panel / 2 + 70, 180], fill=ACCENT + (110,))
    side.alpha_composite(glow.filter(ImageFilter.GaussianBlur(34)))
    img.alpha_composite(side, (0, 0))
    paste_with_shadow(img, icon(84), ((panel - 84) // 2, 66), radius=6, offset=3, alpha=120)
    d = ImageDraw.Draw(img)
    centered_text(d, panel / 2, 166, "Ferrocord", font("SemiBold", 21), PAPER)
    for i, line in enumerate(["A fast, open-source", "Discord client"]):
        centered_text(d, panel / 2, 196 + i * 16, line, font("Regular", 11), (0xC9, 0xCB, 0xD1))
    d.rectangle([panel - 3, 0, panel - 1, h], fill=ACCENT)
    d.text((14, h - 24), "BSD-3-Clause", font=font("Regular", 9), fill=(0x8A, 0x8D, 0x96))
    img.convert("RGB").save(out, "BMP")


def dmg_background(out: Path, scale: int) -> None:
    # Keep in sync with packaging/macos/dmg-settings.py (window 660x400, icons at y=205).
    w, h = 660 * scale, 400 * scale
    img = gradient((w, h), PAPER, PAPER_WARM, diagonal=True).convert("RGBA")
    d = ImageDraw.Draw(img)
    s = scale
    d.rectangle([0, 0, w, 4 * s], fill=ACCENT)
    centered_text(d, w / 2, 34 * s, "Install Ferrocord", font("SemiBold", 26 * s), INK)
    centered_text(d, w / 2, 72 * s, "Drag the app onto the Applications folder", font("Regular", 14 * s), MUTED)
    # Arrow between the app (x=170) and Applications (x=490) icons.
    y = 205 * s
    x0, x1 = 248 * s, 404 * s
    for i in range(x0, x1 - 18 * s, 12 * s):
        d.rounded_rectangle([i, y - 2 * s, i + 6 * s, y + 2 * s], radius=2 * s, fill=ACCENT + (255,))
    d.polygon([(x1, y), (x1 - 18 * s, y - 11 * s), (x1 - 18 * s, y + 11 * s)], fill=ACCENT)
    centered_text(d, w / 2, 350 * s, TAGLINE, font("Regular", 11 * s), (0xA0, 0xA3, 0xAB))
    img.convert("RGB").save(out, "PNG", optimize=True, dpi=(72 * scale, 72 * scale))


def main() -> None:
    win = ROOT / "packaging/windows/wix"
    mac = ROOT / "packaging/macos/dmg"
    win.mkdir(parents=True, exist_ok=True)
    mac.mkdir(parents=True, exist_ok=True)
    wix_banner(win / "banner.bmp")
    wix_dialog(win / "dialog.bmp")
    dmg_background(mac / "background.png", 1)
    dmg_background(mac / "background@2x.png", 2)
    for p in [win / "banner.bmp", win / "dialog.bmp", mac / "background.png", mac / "background@2x.png"]:
        print(p.relative_to(ROOT))


if __name__ == "__main__":
    main()
