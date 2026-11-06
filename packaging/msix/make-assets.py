"""Generates the MSIX logo assets and the Store listing images.

The Tessera icon is three rectangles, so it is drawn as vector graphics at
every size. Sizes that `tessera.ico` contains (hand-tuned small frames) are
taken from the ico instead.

The output is committed, so CI needs no Python. Run again after changing
the icon:

    pip install pillow
    python packaging/msix/make-assets.py
"""

from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[2]
ICO = ROOT / "crates" / "tessera" / "tessera.ico"
ASSETS = ROOT / "packaging" / "msix" / "Assets"
STORE_IMAGES = ROOT / "packaging" / "store" / "images"

BLUE = (98, 160, 234, 255)
LIGHT = (200, 220, 245, 255)
# Rectangles of the icon on its 256 × 256 grid: (left, top, right, bottom).
RECTS = [
    ((16, 16, 120, 240), BLUE),
    ((136, 16, 240, 120), LIGHT),
    ((136, 136, 240, 240), LIGHT),
]
SUPERSAMPLE = 8


def draw_icon(icon_size: int, canvas: tuple[int, int], background=(0, 0, 0, 0)) -> Image.Image:
    """Icon `icon_size` pixels wide, centered on a canvas; edges anti-aliased by area."""
    w, h = canvas[0] * SUPERSAMPLE, canvas[1] * SUPERSAMPLE
    image = Image.new("RGBA", (w, h), background)
    draw = ImageDraw.Draw(image)
    scale = icon_size * SUPERSAMPLE / 256
    x0, y0 = (w - icon_size * SUPERSAMPLE) / 2, (h - icon_size * SUPERSAMPLE) / 2
    for (left, top, right, bottom), color in RECTS:
        draw.rectangle(
            (
                round(x0 + left * scale),
                round(y0 + top * scale),
                round(x0 + right * scale) - 1,
                round(y0 + bottom * scale) - 1,
            ),
            fill=color,
        )
    return image.resize(canvas, Image.Resampling.BOX)


def ico_frames() -> dict[int, Image.Image]:
    ico = Image.open(ICO)
    frames = {}
    for width, _ in ico.info["sizes"]:
        ico.size = (width, width)
        ico.load()
        frames[width] = ico.convert("RGBA").copy()
    return frames


def square(size: int, frames: dict[int, Image.Image]) -> Image.Image:
    """Full-size icon: the hand-tuned ico frame if there is one, else vector."""
    return frames.get(size) or draw_icon(size, (size, size))


def main() -> None:
    ASSETS.mkdir(parents=True, exist_ok=True)
    STORE_IMAGES.mkdir(parents=True, exist_ok=True)
    for old in ASSETS.glob("*.png"):
        old.unlink()
    frames = ico_frames()

    def save(image: Image.Image, name: str) -> None:
        image.save(ASSETS / name, optimize=True)

    # App list, taskbar, Start, Settings: scale variants and exact target sizes.
    for scale in (100, 125, 150, 200, 400):
        save(square(round(44 * scale / 100), frames), f"Square44x44Logo.scale-{scale}.png")
        save(square(round(50 * scale / 100), frames), f"StoreLogo.scale-{scale}.png")
        tile = round(150 * scale / 100)
        save(draw_icon(round(tile * 0.66), (tile, tile)), f"Square150x150Logo.scale-{scale}.png")
    for size in (16, 20, 24, 30, 32, 36, 40, 48, 60, 64, 72, 80, 96, 256):
        image = square(size, frames)
        for suffix in ("", "_altform-unplated", "_altform-lightunplated"):
            save(image, f"Square44x44Logo.targetsize-{size}{suffix}.png")

    # Partner Center store listing: square logo and 1:1 box art.
    draw_icon(300, (300, 300)).save(STORE_IMAGES / "logo-300x300.png", optimize=True)
    draw_icon(720, (1080, 1080), background=(30, 42, 58, 255)).save(
        STORE_IMAGES / "boxart-1080x1080.png", optimize=True
    )
    draw_icon(1440, (2160, 2160), background=(30, 42, 58, 255)).save(
        STORE_IMAGES / "boxart-2160x2160.png", optimize=True
    )
    print(f"{len(list(ASSETS.glob('*.png')))} assets in {ASSETS}")


if __name__ == "__main__":
    main()
