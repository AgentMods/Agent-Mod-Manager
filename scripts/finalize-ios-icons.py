"""Remove residual alpha from generated iOS icons (requires Pillow)."""

from pathlib import Path

from PIL import Image


folder = Path(__file__).resolve().parents[1] / "src-tauri" / "icons" / "ios"
for path in folder.glob("*.png"):
    with Image.open(path) as source:
        icon = source.convert("RGBA")
    canvas = Image.new("RGBA", icon.size, "#FFFFFF")
    canvas.alpha_composite(icon)
    canvas.convert("RGB").save(path)
