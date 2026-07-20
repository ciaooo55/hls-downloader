from pathlib import Path

from PIL import Image, ImageDraw


ROOT = Path(__file__).resolve().parent.parent
ASSET_DIR = ROOT / "assets"
FRONTEND_PUBLIC = ROOT / "frontend" / "public"
EXTENSION_PUBLIC = ROOT / "extension" / "public"
CANVAS_SIZE = 1024


def build_icon() -> Image.Image:
    image = Image.new("RGBA", (CANVAS_SIZE, CANVAS_SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)

    draw.rounded_rectangle(
        (36, 36, 988, 988),
        radius=218,
        fill=(239, 252, 255, 255),
        outline=(14, 165, 233, 255),
        width=22,
    )
    draw.rounded_rectangle((152, 170, 350, 226), radius=28, fill=(250, 204, 21, 255))
    draw.rounded_rectangle((674, 170, 872, 226), radius=28, fill=(251, 113, 133, 255))

    blue = (14, 165, 233, 255)
    draw.rounded_rectangle((453, 164, 571, 574), radius=58, fill=blue)
    draw.polygon(((298, 500), (512, 724), (726, 500)), fill=blue)

    draw.rounded_rectangle((204, 742, 820, 842), radius=50, fill=(255, 255, 255, 255), outline=(16, 185, 129, 255), width=18)
    draw.rounded_rectangle((292, 775, 732, 809), radius=17, fill=(16, 185, 129, 255))
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
    source.save(
        ASSET_DIR / "app-icon.ico",
        format="ICO",
        sizes=[(16, 16), (20, 20), (24, 24), (32, 32), (40, 40), (48, 48), (64, 64), (128, 128), (256, 256)],
    )


if __name__ == "__main__":
    main()
