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

    # A restrained dark app tile that belongs with the native Compose shell.
    # The previous pastel status lights looked playful at 512 px and became
    # visual noise at 16 px; this mark stays legible in the taskbar and toolbar.
    draw.rounded_rectangle(
        (36, 36, 988, 988),
        radius=218,
        fill=(24, 29, 38, 255),
        outline=(57, 68, 84, 255),
        width=18,
    )
    draw.rounded_rectangle((68, 68, 956, 956), radius=190, outline=(35, 42, 54, 255), width=12)

    # Two short transfer lanes add motion without competing with the glyph.
    draw.rounded_rectangle((186, 252, 342, 290), radius=19, fill=(59, 130, 246, 210))
    draw.rounded_rectangle((682, 252, 838, 290), radius=19, fill=(139, 92, 246, 210))

    cyan = (56, 189, 248, 255)
    cyan_shadow = (14, 116, 144, 170)
    draw.rounded_rectangle((444, 188, 580, 574), radius=66, fill=cyan_shadow)
    draw.polygon(((294, 500), (512, 738), (730, 500)), fill=cyan_shadow)
    draw.rounded_rectangle((428, 172, 564, 558), radius=66, fill=cyan)
    draw.polygon(((278, 484), (496, 722), (714, 484)), fill=cyan)

    # The open tray reads clearly as a destination even at favicon sizes.
    mint = (52, 211, 153, 255)
    draw.line(((226, 746), (226, 814), (798, 814), (798, 746)), fill=(9, 78, 74, 210), width=80, joint="curve")
    draw.line(((210, 730), (210, 798), (782, 798), (782, 730)), fill=mint, width=64, joint="curve")
    draw.rounded_rectangle((326, 766, 666, 814), radius=24, fill=(224, 255, 247, 255))
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
