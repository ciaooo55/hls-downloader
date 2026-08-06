from pathlib import Path

from PIL import Image, ImageDraw


ROOT = Path(__file__).resolve().parent.parent
ASSET_DIR = ROOT / "assets"
FRONTEND_PUBLIC = ROOT / "frontend" / "public"
EXTENSION_PUBLIC = ROOT / "extension" / "public"
CANVAS_SIZE = 1024
EXTENSION_ICON_SIZES = (16, 32, 48, 128)


def build_icon() -> Image.Image:
    image = Image.new("RGBA", (CANVAS_SIZE, CANVAS_SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)

    # Bright, flat product mark.  Keep the construction deliberately simple:
    # the same silhouette has to survive Windows taskbar rendering and a 16 px
    # browser toolbar without relying on glow, texture or hairline details.
    draw.rounded_rectangle(
        (44, 44, 980, 980),
        radius=224,
        fill=(247, 251, 255, 255),
        outline=(186, 224, 255, 255),
        width=20,
    )

    blue = (37, 99, 235, 255)
    cyan = (14, 165, 233, 255)
    coral = (255, 107, 74, 255)
    tile = (247, 251, 255, 255)

    # One combined download/media glyph: the play cut-out makes the product's
    # media focus clear without adding a second competing symbol.
    draw.rounded_rectangle((420, 178, 604, 548), radius=86, fill=blue)
    draw.polygon(((250, 448), (774, 448), (512, 736)), fill=blue)
    draw.polygon(((466, 374), (466, 568), (620, 471)), fill=tile)

    # A cyan destination tray and one warm progress accent keep the mark lively
    # while remaining readable when the browser asks for a 16 px icon.
    draw.line(((230, 744), (230, 824), (794, 824), (794, 744)), fill=cyan, width=68, joint="curve")
    draw.rounded_rectangle((382, 790, 642, 846), radius=28, fill=coral)
    return image


def main() -> None:
    ASSET_DIR.mkdir(parents=True, exist_ok=True)
    FRONTEND_PUBLIC.mkdir(parents=True, exist_ok=True)
    EXTENSION_PUBLIC.mkdir(parents=True, exist_ok=True)

    source = build_icon()
    png = source.resize((512, 512), Image.Resampling.LANCZOS)
    png.save(ASSET_DIR / "app-icon.png", optimize=True)
    png.save(FRONTEND_PUBLIC / "app-icon.png", optimize=True)
    png.save(EXTENSION_PUBLIC / "icon.png", optimize=True)
    for size in EXTENSION_ICON_SIZES:
        sized = source.resize((size, size), Image.Resampling.LANCZOS)
        sized.save(EXTENSION_PUBLIC / f"icon-{size}.png", optimize=True)
    source.save(
        ASSET_DIR / "app-icon.ico",
        format="ICO",
        sizes=[(16, 16), (20, 20), (24, 24), (32, 32), (40, 40), (48, 48), (64, 64), (128, 128), (256, 256)],
    )


if __name__ == "__main__":
    main()
