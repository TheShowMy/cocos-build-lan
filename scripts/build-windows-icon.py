from pathlib import Path

from PIL import Image, ImageDraw


SCALE = 4
SIZE = 256 * SCALE


def point(x: int, y: int) -> tuple[int, int]:
    return x * SCALE, y * SCALE


image = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
draw = ImageDraw.Draw(image)
draw.rounded_rectangle(
    (*point(12, 12), *point(244, 244)),
    radius=48 * SCALE,
    fill="#10233f",
)
draw.polygon([point(128, 48), point(184, 78), point(128, 108), point(72, 78)], fill="#38bdf8")
draw.polygon([point(72, 78), point(128, 108), point(128, 172), point(72, 142)], fill="#1683c7")
draw.polygon([point(184, 78), point(128, 108), point(128, 172), point(184, 142)], fill="#22c98a")
for start, end in [((128, 172), (128, 195)), ((92, 184), (64, 202)), ((164, 184), (192, 202))]:
    draw.line([point(*start), point(*end)], fill="#91f2cb", width=12 * SCALE)
for x, y in [(128, 208), (52, 210), (204, 210)]:
    draw.ellipse((*point(x - 16, y - 16), *point(x + 16, y + 16)), fill="#e8fff7")

output = Path(__file__).resolve().parents[1] / "assets" / "windows" / "app.ico"
image.save(output, format="ICO", sizes=[(size, size) for size in (16, 24, 32, 48, 64, 128, 256)])
print(output)
