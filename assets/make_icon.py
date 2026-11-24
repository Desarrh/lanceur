"""Génère l'icône de l'app « Lanceur » (PNG + ICO multi-tailles).

Carré arrondi bleu nuit, une barre de saisie crème avec un chevron bleu « › ». Pas de texte.
Usage : python assets/make_icon.py
"""
from pathlib import Path
from PIL import Image, ImageDraw

NAVY = (19, 34, 58, 255)
CREAM = (242, 230, 204, 255)
BLUE = (111, 168, 255, 255)
S, SS = 1024, 4
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]


def draw(n: int) -> Image.Image:
    k = SS * n / S
    W = int(n * SS)
    img = Image.new("RGBA", (W, W), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    s = lambda v: int(round(v * k))
    d.rounded_rectangle([0, 0, s(S) - 1, s(S) - 1], radius=s(220), fill=NAVY)
    d.rounded_rectangle([s(120), s(392), s(904), s(632)], radius=s(120), fill=CREAM)
    w = s(56)
    d.line([(s(250), s(440)), (s(340), s(512)), (s(250), s(584))], fill=BLUE, width=w, joint="curve")
    d.line([(s(430), s(512)), (s(760), s(512))], fill=(19, 34, 58, 90), width=s(30))
    return img.resize((n, n), Image.LANCZOS)


out = Path(__file__).resolve().parent
draw(256).save(out / "icon.png")
draw(64).save(out / "icon64.png")
draw(32).save(out / "icon32.png")
frames = [draw(n) for n in ICO_SIZES]
frames[-1].save(out / "icon.ico", format="ICO", sizes=[(n, n) for n in ICO_SIZES], append_images=frames[:-1])
print("ok")
