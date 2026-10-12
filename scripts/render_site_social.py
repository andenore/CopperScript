"""Regenerate the optional share image with Pillow and local system fonts."""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont


def font(size: int, serif: bool = False, mono: bool = False) -> ImageFont.FreeTypeFont:
    candidates = (
        ["C:/Windows/Fonts/consola.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"] if mono else
        ["C:/Windows/Fonts/georgia.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf"] if serif else
        ["C:/Windows/Fonts/segoeui.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"]
    )
    for path in candidates:
        if Path(path).exists():
            return ImageFont.truetype(path, size)
    raise SystemExit("Install Georgia/Segoe UI/Consolas or DejaVu system fonts before regenerating.")


def main() -> None:
    image = Image.new("RGB", (1200, 630), "#f6f4ee")
    draw = ImageDraw.Draw(image)
    logo_path = Path(__file__).resolve().parents[1] / "pcbir/editor/assets/copper-logo.png"
    with Image.open(logo_path) as source:
        logo = source.convert("RGBA").resize((42, 42), Image.Resampling.LANCZOS)
    image.paste(logo, (54, 47), logo)
    draw.text((107, 47), "CopperScript", font=font(30), fill="#252b29")
    draw.line((54, 112, 1146, 112), fill="#d9dbd0", width=2)
    draw.text((54, 166), "PCB DESIGN AS CODE", font=font(18, mono=True), fill="#ad4b28")
    draw.text((50, 217), "Hardware intent.", font=font(64, serif=True), fill="#252b29")
    draw.text((50, 298), "Written in code.", font=font(64, serif=True), fill="#ad4b28")
    draw.text((54, 414), "Circuits. Mechanics. Constraints. Inspectable copper.", font=font(22), fill="#626962")
    draw.rectangle((731, 165, 1146, 479), fill="#202b29")
    draw.text((754, 188), "tracker.copper / excerpt", font=font(16, mono=True), fill="#b7c1b3")
    draw.line((731, 227, 1146, 227), fill="#43504b", width=1)
    draw.multiline_text((754, 252), 'component C_MCU: CAPACITOR {\n  value = 100nF;\n  role = decoupling;\n  decouples = "U_MCU.VDD";\n}', font=font(17, mono=True), fill="#c6d7b0", spacing=13)
    draw.line((54, 538, 1146, 538), fill="#d9dbd0", width=2)
    draw.text((54, 562), "Open source · mechanical editor · typed PCB language · KiCad export", font=font(20), fill="#626962")
    target = Path(__file__).resolve().parents[1] / "site/assets/social-preview.png"
    target.parent.mkdir(parents=True, exist_ok=True)
    image.save(target)
    print(f"Wrote {target}")


if __name__ == "__main__":
    main()
