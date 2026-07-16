from pathlib import Path

from PIL import Image, ImageDraw


ROOT = Path(__file__).resolve().parent.parent
ASSET_DIR = ROOT / "assets"
FRONTEND_PUBLIC = ROOT / "frontend" / "public"
CANVAS_SIZE = 1024


def build_icon() -> Image.Image:
    image = Image.new("RGBA", (CANVAS_SIZE, CANVAS_SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)

    draw.rounded_rectangle(
        (36, 36, 988, 988),
        radius=218,
        fill=(31, 36, 43, 255),
        outline=(72, 82, 94, 255),
        width=18,
    )
    draw.rounded_rectangle((154, 170, 350, 226), radius=28, fill=(48, 155, 218, 255))
    draw.rounded_rectangle((674, 170, 870, 226), radius=28, fill=(221, 166, 61, 255))

    green = (57, 201, 132, 255)
    draw.rounded_rectangle((453, 174, 571, 574), radius=58, fill=green)
    draw.polygon(((302, 500), (512, 720), (722, 500)), fill=green)

    draw.rounded_rectangle((210, 742, 814, 838), radius=48, fill=(235, 240, 245, 255))
    draw.rounded_rectangle((292, 768, 732, 812), radius=22, fill=(65, 75, 85, 255))
    return image


def main() -> None:
    ASSET_DIR.mkdir(parents=True, exist_ok=True)
    FRONTEND_PUBLIC.mkdir(parents=True, exist_ok=True)

    source = build_icon()
    png = source.resize((512, 512), Image.Resampling.LANCZOS)
    png.save(ASSET_DIR / "app-icon.png", optimize=True)
    png.save(FRONTEND_PUBLIC / "app-icon.png", optimize=True)
    source.save(
        ASSET_DIR / "app-icon.ico",
        format="ICO",
        sizes=[(16, 16), (20, 20), (24, 24), (32, 32), (40, 40), (48, 48), (64, 64), (128, 128), (256, 256)],
    )


if __name__ == "__main__":
    main()
