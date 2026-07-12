# /// script
# requires-python = ">=3.11"
# dependencies = ["pillow>=10.0", "numpy>=1.26"]
# ///
"""Hypercolor brand asset build pipeline.

Reads source/ and writes master/, mask/, and derived/.
Idempotent — safe to re-run.
"""
from __future__ import annotations

import shutil
import sys
from pathlib import Path
from PIL import Image, ImageChops, ImageFilter
import numpy as np

# Windows console defaults to cp1252; force UTF-8 so arrows/checkmarks print.
if sys.stdout.encoding and sys.stdout.encoding.lower() != "utf-8":
    sys.stdout.reconfigure(encoding="utf-8")
    sys.stderr.reconfigure(encoding="utf-8")

BRAND = Path(__file__).parent
REPO_ROOT = BRAND.parent.parent
SOURCE = BRAND / "source"
MASTER = BRAND / "master"
MASK = BRAND / "mask"
DERIVED = BRAND / "derived"
APP_ICON_DIR = REPO_ROOT / "crates" / "hypercolor-app" / "icons"

INSTALLER_APP_ASSETS = ("installer.ico", "nsis-header.bmp", "nsis-sidebar.bmp")

AI_PETAL_SOURCES = {
    "top": SOURCE / "petal-ai-top.png",
    "left": SOURCE / "petal-ai-left.png",
    "right": SOURCE / "petal-ai-right.png",
}

# Brand colors (R, G, B)
VOID_BLACK = (10, 6, 18)
ELECTRIC_MAGENTA = (225, 53, 255)
NEON_CYAN = (128, 255, 234)
CORAL_PINK = (255, 106, 193)


# ─── utilities ─────────────────────────────────────────────────────────────

def tight_crop(im: Image.Image, padding_pct: float = 0.025, threshold: int = 6) -> Image.Image:
    """Trim to content bbox with proportional padding. Handles RGB/RGBA."""
    if im.mode == "RGBA":
        alpha = im.split()[-1]
        m = alpha.point(lambda v: 255 if v > threshold else 0)
    else:
        gray = im.convert("L")
        m = gray.point(lambda v: 255 if v > threshold else 0)
    bbox = m.getbbox()
    if not bbox:
        return im
    w, h = im.size
    pad = int(max(w, h) * padding_pct)
    x0 = max(0, bbox[0] - pad)
    y0 = max(0, bbox[1] - pad)
    x1 = min(w, bbox[2] + pad)
    y1 = min(h, bbox[3] + pad)
    return im.crop((x0, y0, x1, y1))


def black_to_alpha(im: Image.Image, gamma: float = 1.0) -> Image.Image:
    """RGB-on-black → RGBA using max-channel luminance as alpha.

    Preserves color brightness; dark areas become transparent. Use gamma > 1
    to push midtones more opaque.
    """
    if im.mode != "RGB":
        im = im.convert("RGB")
    r, g, b = im.split()
    alpha = ImageChops.lighter(ImageChops.lighter(r, g), b)
    if gamma != 1.0:
        lut = [min(255, int(255 * ((v / 255) ** (1.0 / gamma)))) for v in range(256)]
        alpha = alpha.point(lut)
    return Image.merge("RGBA", (r, g, b, alpha))


def to_alpha_mask(im: Image.Image) -> Image.Image:
    """Extract grayscale mask. RGBA → alpha channel; RGB → luminance."""
    if im.mode == "RGBA":
        return im.split()[-1]
    return im.convert("L")


def mask_to_css_ready(gray: Image.Image) -> Image.Image:
    """Convert a grayscale mask into an RGBA "white with alpha" PNG so CSS
    `mask-image` works with the default `mask-mode: match-source`.

    The shipped pattern across browsers: opaque white pixels where the mask
    should show, transparent elsewhere, with the luminance value driving alpha.
    This makes the mask behave correctly regardless of mask-mode default.
    """
    if gray.mode != "L":
        gray = gray.convert("L")
    w, h = gray.size
    white = Image.new("L", (w, h), 255)
    return Image.merge("RGBA", (white, white, white, gray))


def pad_to_square(im: Image.Image, bg=(0, 0, 0, 0)) -> Image.Image:
    """Pad to square with transparent (or given) bg, content centered."""
    w, h = im.size
    sq = max(w, h)
    mode = "RGBA" if im.mode == "RGBA" else "RGB"
    bg_use = bg if mode == "RGBA" else bg[:3]
    canvas = Image.new(mode, (sq, sq), bg_use)
    if im.mode == "RGBA":
        canvas.paste(im, ((sq - w) // 2, (sq - h) // 2), im)
    else:
        canvas.paste(im, ((sq - w) // 2, (sq - h) // 2))
    return canvas


def radial_gradient(size: tuple[int, int], inner=(40, 20, 80), outer=(8, 4, 18)) -> Image.Image:
    """Generate a radial gradient bg for installer art / OG."""
    w, h = size
    cx, cy = w / 2, h / 2
    max_r = (cx ** 2 + cy ** 2) ** 0.5
    y, x = np.indices((h, w), dtype=np.float32)
    r = np.sqrt((x - cx) ** 2 + (y - cy) ** 2) / max_r
    r = np.clip(r, 0, 1)
    inner_arr = np.array(inner, dtype=np.float32)
    outer_arr = np.array(outer, dtype=np.float32)
    grad = inner_arr * (1 - r[..., None]) + outer_arr * r[..., None]
    return Image.fromarray(grad.astype(np.uint8), "RGB")


# ─── luminary installer compositing ──────────────────────────────────────────
# The Windows installer only gives us a 164x314 sidebar and a 150x57 header to
# carry the brand, and Windows DPI-upscales both. So the art is composed
# supersampled then downsampled (crisp edges), and built from soft glow + bloom
# rather than fine detail so it stays gorgeous when the installer stretches it.

LUMA_SS = 4  # supersample factor for installer art
LUMA_SEED = 0xC0107  # deterministic sparkle/dither so rebuilds don't churn

DEEP_VIOLET = (138, 72, 255)
HORIZON_MAGENTA = (225, 53, 255)


def _to_pil(buf: np.ndarray) -> Image.Image:
    return Image.fromarray(np.clip(buf, 0, 255).astype(np.uint8), "RGB")


def _fit_width(im: Image.Image, target_w: int) -> Image.Image:
    w, h = im.size
    return im.resize((target_w, max(1, round(h * target_w / w))), Image.LANCZOS)


def _fit_height(im: Image.Image, target_h: int) -> Image.Image:
    w, h = im.size
    return im.resize((max(1, round(w * target_h / h)), target_h), Image.LANCZOS)


def _grad_v(w: int, h: int, stops: list[tuple[float, tuple[int, int, int]]]) -> np.ndarray:
    """Vertical multi-stop gradient → float HxWx3."""
    ys = np.linspace(0.0, 1.0, h)
    pos = [s[0] for s in stops]
    col = np.stack([np.interp(ys, pos, [s[1][c] for s in stops]) for c in range(3)], axis=1)
    return np.repeat(col[:, None, :], w, axis=1).astype(np.float32)


def _grad_h(w: int, h: int, stops: list[tuple[float, tuple[int, int, int]]]) -> np.ndarray:
    """Horizontal multi-stop gradient → float HxWx3."""
    xs = np.linspace(0.0, 1.0, w)
    pos = [s[0] for s in stops]
    row = np.stack([np.interp(xs, pos, [s[1][c] for s in stops]) for c in range(3)], axis=1)
    return np.repeat(row[None, :, :], h, axis=0).astype(np.float32)


def _radial(w: int, h: int, cx: float, cy: float, radius: float, power: float = 2.0) -> np.ndarray:
    """Smooth radial falloff (1.0 at center → 0.0 at radius) → float HxW."""
    y, x = np.indices((h, w), dtype=np.float32)
    d = np.sqrt((x - cx) ** 2 + (y - cy) ** 2) / radius
    return np.clip(1.0 - d, 0.0, 1.0) ** power


def _add_glow(buf: np.ndarray, fall: np.ndarray, color: tuple[int, int, int], gain: float) -> None:
    buf += fall[..., None] * np.array(color, np.float32) * gain


def _add_color_bloom(buf: np.ndarray, layer: Image.Image, passes: list[tuple[float, float]]) -> None:
    """Bleed a layer's own color outward as a luminous halo (additive)."""
    arr = np.asarray(layer, np.float32)
    prem = Image.fromarray((arr[..., :3] * (arr[..., 3:4] / 255.0)).astype(np.uint8), "RGB")
    for blur, gain in passes:
        buf += np.asarray(prem.filter(ImageFilter.GaussianBlur(blur)), np.float32) * gain


def _over(buf: np.ndarray, fg: Image.Image, xy: tuple[int, int]) -> None:
    """Alpha-composite an RGBA layer onto a float buffer at top-left xy."""
    fx, fy = xy
    fw, fh = fg.size
    h, w = buf.shape[:2]
    x0, y0 = max(0, fx), max(0, fy)
    x1, y1 = min(w, fx + fw), min(h, fy + fh)
    if x0 >= x1 or y0 >= y1:
        return
    f = np.asarray(fg, np.float32)[y0 - fy:y1 - fy, x0 - fx:x1 - fx]
    a = f[..., 3:4] / 255.0
    buf[y0:y1, x0:x1] = f[..., :3] * a + buf[y0:y1, x0:x1] * (1.0 - a)


def _vignette(buf: np.ndarray, strength: float) -> None:
    h, w = buf.shape[:2]
    fall = _radial(w, h, w / 2, h * 0.42, (w ** 2 + h ** 2) ** 0.5 * 0.5, power=1.6)
    buf *= (1.0 - strength) + strength * fall[..., None]


def _sparkles(buf: np.ndarray, rng: np.random.Generator, n: int, ss: int) -> None:
    """Scatter soft additive light motes — the app's electric energy as particles."""
    h, w = buf.shape[:2]
    palette = [ELECTRIC_MAGENTA, NEON_CYAN, CORAL_PINK, (255, 255, 255)]
    weights = [0.34, 0.30, 0.30, 0.06]
    for _ in range(n):
        x = rng.uniform(0.05, 0.95) * w
        y = rng.uniform(0.04, 0.96) * h
        r = rng.uniform(0.6, 2.4) * ss
        color = palette[rng.choice(len(palette), p=weights)]
        _add_glow(buf, _radial(w, h, x, y, r * 3.2, 2.6), color, rng.uniform(0.45, 1.0))


def _luminary_vertical(
    w: int,
    h: int,
    *,
    mark_wf: float,
    mark_y: float,
    wm_wf: float,
    wm_y: float,
    sparkles: int,
    ss: int = LUMA_SS,
    seed: int = LUMA_SEED,
) -> Image.Image:
    """Glowing triskelion over a nebula field with a glowing wordmark below."""
    big_w, big_h = w * ss, h * ss
    rng = np.random.default_rng(seed)
    buf = _grad_v(big_w, big_h, [
        (0.00, (12, 7, 22)),
        (0.34, (27, 13, 52)),
        (0.62, (17, 9, 34)),
        (1.00, (9, 5, 16)),
    ])
    _add_glow(buf, _radial(big_w, big_h, big_w * 0.30, big_h * 0.28, big_w * 0.95, 1.7), DEEP_VIOLET, 0.10)
    _add_glow(buf, _radial(big_w, big_h, big_w * 0.76, big_h * 0.48, big_w * 0.80, 1.8), NEON_CYAN, 0.045)
    _add_glow(buf, _radial(big_w, big_h, big_w * 0.26, big_h * 0.70, big_w * 0.75, 1.8), CORAL_PINK, 0.05)
    _vignette(buf, 0.50)
    _add_glow(buf, _radial(big_w, big_h, big_w * 0.5, big_h * (mark_y + 0.16), big_w * 0.9, 1.5),
              ELECTRIC_MAGENTA, 0.16)

    mark = _fit_width(Image.open(MASTER / "mark-color.png").convert("RGBA"), int(big_w * mark_wf))
    mlayer = Image.new("RGBA", (big_w, big_h), (0, 0, 0, 0))
    mlayer.paste(mark, ((big_w - mark.width) // 2, int(big_h * mark_y)), mark)
    _add_color_bloom(buf, mlayer, [(big_w * 0.05, 0.55), (big_w * 0.11, 0.40), (big_w * 0.20, 0.22)])
    _over(buf, mlayer, (0, 0))

    _sparkles(buf, rng, sparkles, ss)

    wm = _fit_width(Image.open(MASTER / "wordmark-glow-color.png").convert("RGBA"), int(big_w * wm_wf))
    wlayer = Image.new("RGBA", (big_w, big_h), (0, 0, 0, 0))
    wlayer.paste(wm, ((big_w - wm.width) // 2, int(big_h * wm_y)), wm)
    _add_color_bloom(buf, wlayer, [(big_w * 0.04, 0.45), (big_w * 0.09, 0.28)])
    _over(buf, wlayer, (0, 0))

    _add_glow(buf, _radial(big_w, big_h, big_w * 0.5, big_h * 1.02, big_w * 0.9, 1.5), HORIZON_MAGENTA, 0.12)
    buf += rng.normal(0.0, 1.4, buf.shape)
    return _to_pil(buf).resize((w, h), Image.LANCZOS)


def _luminary_horizontal(
    w: int,
    h: int,
    *,
    lockup_wf: float = 0.84,
    underline: bool = True,
    ss: int = LUMA_SS,
) -> Image.Image:
    """Horizontal lockup glowing on a dark gradient chip with a neon underline."""
    big_w, big_h = w * ss, h * ss
    buf = _grad_h(big_w, big_h, [(0.0, (10, 6, 20)), (0.5, (23, 11, 44)), (1.0, (10, 6, 20))])
    _add_glow(buf, _radial(big_w, big_h, big_w * 0.5, big_h * 0.5, big_w * 0.7, 1.6), DEEP_VIOLET, 0.12)
    _vignette(buf, 0.30)

    lock = _fit_width(Image.open(MASTER / "lockup-horizontal-color.png").convert("RGBA"), int(big_w * lockup_wf))
    max_h = int(big_h * 0.64)
    if lock.height > max_h:
        lock = _fit_height(lock, max_h)
    llayer = Image.new("RGBA", (big_w, big_h), (0, 0, 0, 0))
    llayer.paste(lock, ((big_w - lock.width) // 2, (big_h - lock.height) // 2), lock)
    _add_color_bloom(buf, llayer, [(big_w * 0.015, 0.40), (big_w * 0.04, 0.22)])
    _over(buf, llayer, (0, 0))

    if underline:
        line = _grad_h(big_w, big_h, [(0.0, ELECTRIC_MAGENTA), (0.5, NEON_CYAN), (1.0, CORAL_PINK)])
        mask = np.zeros((big_h, big_w), np.float32)
        mask[int(big_h * 0.86):int(big_h * 0.92), int(big_w * 0.06):int(big_w * 0.94)] = 1.0
        soft = Image.fromarray((mask * 255).astype(np.uint8), "L").filter(ImageFilter.GaussianBlur(big_h * 0.04))
        buf += line * (np.asarray(soft, np.float32)[..., None] / 255.0) * 0.85

    return _to_pil(buf).resize((w, h), Image.LANCZOS)


# ─── stage 1: masters ──────────────────────────────────────────────────────

def build_masters() -> None:
    print("\n[1/3] building masters")
    MASTER.mkdir(parents=True, exist_ok=True)

    # vertical lockup color (alpha) — from restored-alpha (has the halo noise but real alpha)
    src = Image.open(SOURCE / "restored-alpha.png")
    tight_crop(src).save(MASTER / "lockup-vertical-color.png")

    # vertical lockup on black (canonical, picked: logo-square-black)
    src = Image.open(SOURCE / "logo-square-black.png")
    tight_crop(src).save(MASTER / "lockup-vertical-on-black.png")

    # vertical lockup on white
    src = Image.open(SOURCE / "logo-square-white.png")
    tight_crop(src).save(MASTER / "lockup-vertical-on-white.png")

    # horizontal lockup on black + alpha derived
    src = Image.open(SOURCE / "logo-horizontal.png")
    cropped = tight_crop(src)
    cropped.save(MASTER / "lockup-horizontal-on-black.png")
    black_to_alpha(cropped).save(MASTER / "lockup-horizontal-color.png")

    # mark (petals only)
    src = Image.open(SOURCE / "neon_triskelion_alpha.png")
    tight_crop(src).save(MASTER / "mark-color.png")
    src = Image.open(SOURCE / "icon-black.png")
    tight_crop(src).save(MASTER / "mark-on-black.png")

    # wordmark (chrome on black + alpha derived + glow variant)
    src = Image.open(SOURCE / "wordmark-white.png")
    cropped = tight_crop(src)
    cropped.save(MASTER / "wordmark-on-black.png")
    black_to_alpha(cropped).save(MASTER / "wordmark-color.png")

    src = Image.open(SOURCE / "wordmark-logo-black-glow.png")
    cropped = tight_crop(src)
    cropped.save(MASTER / "wordmark-glow-on-black.png")
    black_to_alpha(cropped, gamma=1.2).save(MASTER / "wordmark-glow-color.png")

    # wordmark-on-white (pure black-on-white version, useful for light contexts)
    src = Image.open(SOURCE / "hypercolor-bw-mask.png")
    tight_crop(src).save(MASTER / "wordmark-on-white.png")

    # individual petals (top petal in three colors)
    for color in ["magenta", "cyan", "violet"]:
        src = Image.open(SOURCE / f"petal-{color}.png")
        cropped = tight_crop(src)
        cropped.save(MASTER / f"petal-top-{color}-on-black.png")
        black_to_alpha(cropped).save(MASTER / f"petal-top-{color}.png")

    print(f"  → {len(list(MASTER.glob('*.png')))} master files")


# ─── stage 2: masks ────────────────────────────────────────────────────────

def find_trinity_center(mark_arr: np.ndarray) -> tuple[float, float]:
    """Weighted centroid of the mask. The trinity's actual visual center —
    not the image's geometric center, which is thrown off by asymmetric glow
    halos and the bbox of the source PNG."""
    h, w = mark_arr.shape
    weight = mark_arr.astype(np.float64)
    total = weight.sum()
    if total == 0:
        return h / 2.0, w / 2.0
    y, x = np.indices((h, w))
    cy = float((y * weight).sum() / total)
    cx = float((x * weight).sum() / total)
    return cy, cx


def find_petal_rotation(mark_arr: np.ndarray, center: tuple[float, float]) -> float:
    """Find the angular offset (degrees) that makes wedge boundaries fall in
    the actual inter-petal gaps.

    Sweeps candidate rotations from -30° to +30° (3-fold symmetry means
    larger offsets are equivalent), and picks the one minimizing total mask
    intensity along three radial lines at 60°/180°/300° + α.
    """
    cy, cx = center
    h, w = mark_arr.shape

    max_r = min(cy, cx, h - cy, w - cx) * 0.85
    # Skip the very inner radius where all petals converge
    radii = np.arange(max_r * 0.2, max_r, 4.0)

    best_alpha = 0.0
    best_gap_sum = float("inf")

    for alpha_deg in np.arange(-30.0, 30.0, 0.5):
        gap_sum = 0.0
        for gap_offset in (60.0, 180.0, 300.0):
            # Convert clock-style angle to standard math angle
            rad = np.radians(gap_offset + alpha_deg - 90.0)
            ys = (cy + radii * np.sin(rad)).astype(int)
            xs = (cx + radii * np.cos(rad)).astype(int)
            valid = (ys >= 0) & (ys < h) & (xs >= 0) & (xs < w)
            gap_sum += float(mark_arr[ys[valid], xs[valid]].sum())
        if gap_sum < best_gap_sum:
            best_gap_sum = gap_sum
            best_alpha = float(alpha_deg)

    return best_alpha


def segment_mark_to_wedges(
    mark_mask_arr: np.ndarray,
    center: tuple[float, float] | None = None,
    rotation_deg: float = 0.0,
    edge_softness_px: float = 0.0,
) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """Split the mark mask into three angular wedges (top/left/right).

    Hard wedge cuts at 60°/180°/300° offset by rotation_deg. The result is an
    exact partition of the source mask: every alpha pixel belongs to one petal,
    and all outputs keep the source canvas dimensions for CSS layer alignment.
    """
    h, w = mark_mask_arr.shape
    if center is None:
        cy, cx = find_trinity_center(mark_mask_arr)
    else:
        cy, cx = center

    y, x = np.indices((h, w))
    dy = y - cy
    dx = x - cx
    # Clock-style angle: 0 at top, positive going clockwise.
    deg = np.degrees(np.arctan2(dx, -dy))
    deg = (deg - rotation_deg + 180.0) % 360.0 - 180.0

    top_wedge = (deg >= -60.0) & (deg < 60.0)
    right_wedge = (deg >= 60.0) & (deg < 180.0)
    left_wedge = (deg >= -180.0) & (deg < -60.0)

    top = np.where(top_wedge, mark_mask_arr, 0).astype(np.uint8)
    right = np.where(right_wedge, mark_mask_arr, 0).astype(np.uint8)
    left = np.where(left_wedge, mark_mask_arr, 0).astype(np.uint8)

    if edge_softness_px > 0:
        # Light blur for anti-aliasing the wedge cuts.
        top = np.array(Image.fromarray(top, "L").filter(ImageFilter.GaussianBlur(edge_softness_px)))
        right = np.array(Image.fromarray(right, "L").filter(ImageFilter.GaussianBlur(edge_softness_px)))
        left = np.array(Image.fromarray(left, "L").filter(ImageFilter.GaussianBlur(edge_softness_px)))

    return top, left, right


def robust_projection_bbox(mask: np.ndarray, min_row: int, min_col: int) -> tuple[int, int, int, int] | None:
    rows = np.where(mask.sum(axis=1) > min_row)[0]
    cols = np.where(mask.sum(axis=0) > min_col)[0]
    if rows.size == 0 or cols.size == 0:
        return None
    return int(cols[0]), int(rows[0]), int(cols[-1]) + 1, int(rows[-1]) + 1


def extract_black_background_alpha(path: Path) -> Image.Image:
    src = Image.open(path).convert("RGB")
    arr = np.asarray(src, dtype=np.float32)
    h, w = arr.shape[:2]
    max_channel = arr.max(axis=-1)
    hard = max_channel > 40
    hard[int(h * 0.84):, int(w * 0.84):] = False

    bbox = robust_projection_bbox(hard, min_row=24, min_col=24)
    if bbox is None:
        raise ValueError(f"could not isolate petal in {path}")

    x0, y0, x1, y1 = bbox
    crop = arr[y0:y1, x0:x1]
    max_channel = crop.max(axis=-1)
    floor = max(12.0, float(np.percentile(max_channel, 2)))
    alpha = np.clip((max_channel - floor) * 255.0 / (255.0 - floor), 0, 255).astype(np.uint8)
    alpha[alpha < 12] = 0
    return Image.fromarray(alpha, "L").filter(ImageFilter.MedianFilter(3))


def petal_wedge_masks(
    shape: tuple[int, int],
    center: tuple[float, float],
    rotation_deg: float,
) -> dict[str, np.ndarray]:
    h, w = shape
    cy, cx = center
    y, x = np.indices((h, w))
    deg = np.degrees(np.arctan2(x - cx, -(y - cy)))
    deg = (deg - rotation_deg + 180.0) % 360.0 - 180.0
    return {
        "top": (deg >= -60.0) & (deg < 60.0),
        "right": (deg >= 60.0) & (deg < 180.0),
        "left": (deg >= -180.0) & (deg < -60.0),
    }


def target_petal_boxes(
    mark_mask_arr: np.ndarray,
    wedges: dict[str, np.ndarray],
) -> dict[str, tuple[int, int, int, int]]:
    h, w = mark_mask_arr.shape
    y, x = np.indices((h, w))
    regions = {
        "top": (x > w * 0.20) & (x < w * 0.80) & (y > h * 0.02) & (y < h * 0.70),
        "left": (x > w * 0.02) & (x < w * 0.55) & (y > h * 0.34) & (y < h * 0.98),
        "right": (x > w * 0.44) & (x < w * 0.98) & (y > h * 0.34) & (y < h * 0.98),
    }
    boxes: dict[str, tuple[int, int, int, int]] = {}
    for name, region in regions.items():
        bbox = robust_projection_bbox(region & wedges[name] & (mark_mask_arr > 31), min_row=8, min_col=8)
        if bbox is None:
            raise ValueError(f"could not find target box for {name} petal")
        boxes[name] = bbox
    return boxes


def segment_mark_to_ai_petals(mark_mask_arr: np.ndarray) -> tuple[np.ndarray, np.ndarray, np.ndarray] | None:
    if not all(path.exists() for path in AI_PETAL_SOURCES.values()):
        return None

    center = find_trinity_center(mark_mask_arr)
    rotation = find_petal_rotation(mark_mask_arr, center)
    wedges = petal_wedge_masks(mark_mask_arr.shape, center, rotation)
    boxes = target_petal_boxes(mark_mask_arr, wedges)
    h, w = mark_mask_arr.shape
    petals: dict[str, np.ndarray] = {}

    print("  using isolated petal clips for segmented masks")
    print(f"  trinity center @ ({center[1]:.0f}, {center[0]:.0f}) of {w}x{h}")
    print(f"  petal rotation = {rotation:+.1f}° (center seams only)")

    for name, path in AI_PETAL_SOURCES.items():
        x0, y0, x1, y1 = boxes[name]
        alpha = extract_black_background_alpha(path)
        resized = alpha.resize((x1 - x0, y1 - y0), Image.LANCZOS)
        canvas = Image.new("L", (w, h), 0)
        canvas.paste(resized, (x0, y0))
        petal = np.where(wedges[name], np.asarray(canvas), 0)
        petals[name] = np.where(mark_mask_arr > 0, petal, 0).astype(np.uint8)

    return petals["top"], petals["left"], petals["right"]


def build_masks() -> None:
    print("\n[2/3] building masks")
    MASK.mkdir(parents=True, exist_ok=True)

    # Full mark mask (petals only). Keep grayscale `mark_mask` in local scope
    # for segmentation, but persist the CSS-ready RGBA variant to disk.
    src = Image.open(MASTER / "mark-color.png")
    mark_mask = to_alpha_mask(src)
    mask_to_css_ready(mark_mask).save(MASK / "mark-mask.png")

    # Vertical lockup mask (mark + wordmark)
    src = Image.open(MASTER / "lockup-vertical-color.png")
    mask_to_css_ready(to_alpha_mask(src)).save(MASK / "lockup-vertical-mask.png")

    # Horizontal lockup mask
    src = Image.open(MASTER / "lockup-horizontal-color.png")
    mask_to_css_ready(to_alpha_mask(src)).save(MASK / "lockup-horizontal-mask.png")

    # Wordmark-only mask (use bw-mask source for pixel-perfect black-on-white)
    bw = Image.open(SOURCE / "hypercolor-bw-mask.png").convert("L")
    # invert: source is black-letters-on-white, we want white-letters-on-black
    bw_inv = ImageChops.invert(bw)
    bw_cropped = tight_crop(bw_inv, threshold=4)
    mask_to_css_ready(bw_cropped if bw_cropped.mode == "L" else bw_cropped.convert("L")).save(MASK / "wordmark-mask.png")

    # Single petal mask (top petal, for rotation-based use)
    src = Image.open(MASTER / "petal-top-cyan.png")
    mask_to_css_ready(to_alpha_mask(src)).save(MASK / "petal-top-mask.png")

    # 3-channel tri-petal mask: R=top, G=left, B=right.
    mark_arr = np.array(mark_mask)
    h, w = mark_arr.shape

    ai_petals = segment_mark_to_ai_petals(mark_arr)
    if ai_petals is None:
        center = find_trinity_center(mark_arr)
        rotation = find_petal_rotation(mark_arr, center)
        print("  isolated petal clips not found; using angular fallback")
        print(f"  trinity center @ ({center[1]:.0f}, {center[0]:.0f}) of {w}x{h}")
        print(f"  petal rotation = {rotation:+.1f}° (wedges land in inter-petal gaps)")
        top, left, right = segment_mark_to_wedges(mark_arr, center=center, rotation_deg=rotation)
    else:
        top, left, right = ai_petals

    # Each as standalone mask (CSS-ready RGBA so mask-image works by default)
    mask_to_css_ready(Image.fromarray(top, "L")).save(MASK / "petal-top-segmented-mask.png")
    mask_to_css_ready(Image.fromarray(left, "L")).save(MASK / "petal-left-segmented-mask.png")
    mask_to_css_ready(Image.fromarray(right, "L")).save(MASK / "petal-right-segmented-mask.png")

    # Packed 3-channel for shader sampling. Stays RGB — each channel carries
    # one mask, so it's not directly usable as a CSS mask-image (use the
    # per-petal files above for CSS).
    tri = np.stack([top, left, right], axis=-1)
    Image.fromarray(tri, "RGB").save(MASK / "petal-mask-tri.png")

    print(f"  → {len(list(MASK.glob('*.png')))} mask files")


# ─── stage 3: derived ──────────────────────────────────────────────────────

def build_app_icons() -> None:
    out = DERIVED / "app-icon"
    out.mkdir(parents=True, exist_ok=True)

    # Master: square, transparent bg, petals centered with safe margin
    mark = Image.open(MASTER / "mark-color.png").convert("RGBA")
    sq = pad_to_square(mark)
    # add ~5% safe margin (Tauri / OS rounded corners eat the outer pixels)
    margin = int(sq.size[0] * 0.05)
    target = sq.size[0] + margin * 2
    canvas = Image.new("RGBA", (target, target), (0, 0, 0, 0))
    canvas.paste(sq, (margin, margin), sq)
    master_icon = canvas.resize((1024, 1024), Image.LANCZOS)

    # Tauri standard names
    master_icon.save(out / "icon.png")
    for s in [32, 128]:
        master_icon.resize((s, s), Image.LANCZOS).save(out / f"{s}x{s}.png")
    master_icon.resize((256, 256), Image.LANCZOS).save(out / "128x128@2x.png")

    # Generic sized set
    for s in [16, 24, 32, 48, 64, 128, 256, 512, 1024]:
        master_icon.resize((s, s), Image.LANCZOS).save(out / f"icon-{s}.png")

    # Windows multi-size ICO (256 base, embeds smaller sizes)
    master_icon.resize((256, 256), Image.LANCZOS).save(
        out / "icon.ico",
        format="ICO",
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )

    print(f"  → app-icon: {len(list(out.glob('*')))} files")


def build_tray() -> None:
    out = DERIVED / "tray"
    out.mkdir(parents=True, exist_ok=True)

    mark = Image.open(MASTER / "mark-color.png").convert("RGBA")
    sq = pad_to_square(mark)
    base = sq.resize((512, 512), Image.LANCZOS)

    # Color variants (Windows tray uses these; 32 is for the Rust embed path)
    for s in [22, 32, 44, 88, 256]:
        base.resize((s, s), Image.LANCZOS).save(out / f"tray-color-{s}.png")

    # Monochrome (macOS template image — alpha-only black silhouette)
    alpha = base.split()[-1]
    mono_full = Image.new("RGBA", base.size, (0, 0, 0, 0))
    black = Image.new("RGBA", base.size, (0, 0, 0, 255))
    mono_full.paste(black, (0, 0), alpha)
    for s in [22, 32, 44, 88]:
        mono_full.resize((s, s), Image.LANCZOS).save(out / f"tray-mono-{s}.png")

    # Status variants — paused is desaturated
    paused = base.convert("HSV")
    h, s_ch, v = paused.split()
    s_ch = s_ch.point(lambda v: v // 3)
    paused = Image.merge("HSV", (h, s_ch, v)).convert("RGBA")
    paused.putalpha(base.split()[-1])
    for s in [22, 32, 44, 88]:
        paused.resize((s, s), Image.LANCZOS).save(out / f"tray-paused-{s}.png")

    # error: red-tinted
    red_tint = Image.new("RGBA", base.size, (255, 70, 70, 255))
    red_tint.putalpha(base.split()[-1])
    err = Image.blend(base, red_tint, 0.7)
    err.putalpha(base.split()[-1])
    for s in [22, 32, 44, 88]:
        err.resize((s, s), Image.LANCZOS).save(out / f"tray-error-{s}.png")

    # Disconnected — same as mono but lighter (gray, low alpha) for "off" state
    gray_tint = Image.new("RGBA", base.size, (140, 140, 160, 200))
    gray_tint.putalpha(alpha)
    disc = Image.blend(base, gray_tint, 0.85)
    disc.putalpha(Image.eval(alpha, lambda v: v * 6 // 10))
    for s in [22, 32, 44, 88]:
        disc.resize((s, s), Image.LANCZOS).save(out / f"tray-disconnected-{s}.png")

    print(f"  → tray: {len(list(out.glob('*')))} files")


def build_favicon() -> None:
    out = DERIVED / "favicon"
    out.mkdir(parents=True, exist_ok=True)

    mark = Image.open(MASTER / "mark-color.png").convert("RGBA")
    sq = pad_to_square(mark)
    base = sq.resize((512, 512), Image.LANCZOS)

    base.save(out / "icon-512.png")
    base.resize((192, 192), Image.LANCZOS).save(out / "icon-192.png")

    # apple-touch-icon: needs solid bg (iOS doesn't add a tile)
    apple = Image.new("RGB", (180, 180), VOID_BLACK)
    apple_mark = base.resize((150, 150), Image.LANCZOS)
    apple.paste(apple_mark, (15, 15), apple_mark)
    apple.save(out / "apple-touch-icon.png")

    # ICO
    base.resize((48, 48), Image.LANCZOS).save(
        out / "favicon.ico",
        format="ICO",
        sizes=[(16, 16), (32, 32), (48, 48)],
    )

    print(f"  → favicon: {len(list(out.glob('*')))} files")


def build_og() -> None:
    out = DERIVED / "og"
    out.mkdir(parents=True, exist_ok=True)

    # 1200x630 default OG
    bg = radial_gradient((1200, 630), inner=(45, 20, 90), outer=VOID_BLACK)
    canvas = bg.convert("RGBA")

    v_lockup = Image.open(MASTER / "lockup-vertical-color.png").convert("RGBA")
    vw, vh = v_lockup.size
    target_h = 520
    target_w = int(vw * target_h / vh)
    v_scaled = v_lockup.resize((target_w, target_h), Image.LANCZOS)
    canvas.paste(v_scaled, ((1200 - target_w) // 2, (630 - target_h) // 2), v_scaled)
    canvas.convert("RGB").save(out / "og-default.png")

    # 1200x1200 square
    bg = radial_gradient((1200, 1200), inner=(45, 20, 90), outer=VOID_BLACK).convert("RGBA")
    target_h = 900
    target_w = int(vw * target_h / vh)
    v_sq = v_lockup.resize((target_w, target_h), Image.LANCZOS)
    bg.paste(v_sq, ((1200 - target_w) // 2, (1200 - target_h) // 2), v_sq)
    bg.convert("RGB").save(out / "og-square.png")

    # 1600x900 (Twitter card landscape)
    bg = radial_gradient((1600, 900), inner=(45, 20, 90), outer=VOID_BLACK).convert("RGBA")
    target_h = 720
    target_w = int(vw * target_h / vh)
    v_lg = v_lockup.resize((target_w, target_h), Image.LANCZOS)
    bg.paste(v_lg, ((1600 - target_w) // 2, (900 - target_h) // 2), v_lg)
    bg.convert("RGB").save(out / "twitter-card.png")

    print(f"  → og: {len(list(out.glob('*')))} files")


def build_installer_win() -> None:
    out = DERIVED / "installer-win"
    out.mkdir(parents=True, exist_ok=True)

    # NSIS welcome/finish hero (164x314) — the first and last thing the user sees.
    _luminary_vertical(164, 314, mark_wf=0.66, mark_y=0.10, wm_wf=0.74, wm_y=0.77, sparkles=54).save(
        out / "nsis-sidebar.bmp", format="BMP"
    )
    # NSIS page header chip (150x57) — branded bar on the inner wizard pages.
    _luminary_horizontal(150, 57).save(out / "nsis-header.bmp", format="BMP")
    # WiX equivalents kept in sync for any MSI build path.
    _luminary_vertical(493, 312, mark_wf=0.30, mark_y=0.12, wm_wf=0.40, wm_y=0.64, sparkles=130).save(
        out / "wix-dialog.bmp", format="BMP"
    )
    _luminary_horizontal(493, 58, lockup_wf=0.50).save(out / "wix-banner.bmp", format="BMP")

    # Installer .ico — crisp transparent petal mark with a safe margin, multi-res.
    mark = Image.open(MASTER / "mark-color.png").convert("RGBA")
    sq = pad_to_square(mark)
    margin = int(sq.width * 0.06)
    canvas = Image.new("RGBA", (sq.width + margin * 2, sq.height + margin * 2), (0, 0, 0, 0))
    canvas.paste(sq, (margin, margin), sq)
    canvas.resize((256, 256), Image.LANCZOS).save(
        out / "installer.ico",
        format="ICO",
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )

    APP_ICON_DIR.mkdir(parents=True, exist_ok=True)
    for asset in INSTALLER_APP_ASSETS:
        shutil.copy2(out / asset, APP_ICON_DIR / asset)

    print(f"  → installer-win: {len(list(out.glob('*')))} files")
    print(f"  → app installer icons: {len(INSTALLER_APP_ASSETS)} files")


def build_social() -> None:
    out = DERIVED / "social"
    out.mkdir(parents=True, exist_ok=True)

    mark = Image.open(MASTER / "mark-color.png").convert("RGBA")
    sq = pad_to_square(mark)

    # Avatar 1024 (Twitter/Mastodon/GitHub profile)
    bg = radial_gradient((1024, 1024), inner=(35, 15, 70), outer=VOID_BLACK).convert("RGBA")
    mark_scaled = sq.resize((780, 780), Image.LANCZOS)
    bg.paste(mark_scaled, (122, 122), mark_scaled)
    bg.convert("RGB").save(out / "avatar-1024.png")

    # Twitter banner 1500x500
    bg = radial_gradient((1500, 500), inner=(35, 15, 70), outer=VOID_BLACK).convert("RGBA")
    h_lockup = Image.open(MASTER / "lockup-horizontal-color.png").convert("RGBA")
    hw, hh = h_lockup.size
    target_h = 280
    target_w = int(hw * target_h / hh)
    h_scaled = h_lockup.resize((target_w, target_h), Image.LANCZOS)
    bg.paste(h_scaled, ((1500 - target_w) // 2, (500 - target_h) // 2), h_scaled)
    bg.convert("RGB").save(out / "twitter-banner.png")

    # GitHub org banner 1280x640
    bg = radial_gradient((1280, 640), inner=(35, 15, 70), outer=VOID_BLACK).convert("RGBA")
    target_h = 360
    target_w = int(hw * target_h / hh)
    h_scaled = h_lockup.resize((target_w, target_h), Image.LANCZOS)
    bg.paste(h_scaled, ((1280 - target_w) // 2, (640 - target_h) // 2), h_scaled)
    bg.convert("RGB").save(out / "github-banner.png")

    # Discord server icon 512
    bg = radial_gradient((512, 512), inner=(35, 15, 70), outer=VOID_BLACK).convert("RGBA")
    mark_scaled = sq.resize((420, 420), Image.LANCZOS)
    bg.paste(mark_scaled, (46, 46), mark_scaled)
    bg.convert("RGB").save(out / "discord-icon-512.png")

    print(f"  → social: {len(list(out.glob('*')))} files")


def build_web() -> None:
    """Web-optimized small variants for marketing-site consumption.

    Masters are >1 MB at 1145+ px; the marketing site only ever displays
    them at <500 px. Without these, `next/image` (which the site runs with
    `unoptimized: true`) downloads megabytes to render a 32 px nav icon.
    """
    out = DERIVED / "web"
    out.mkdir(parents=True, exist_ok=True)

    # Square padded mark for nav / footer / any boxed surface.
    # The 512 covers OG image embedding (Satori renders at ~420 px).
    mark_raw = Image.open(MASTER / "mark-color.png").convert("RGBA")
    mark_sq = pad_to_square(mark_raw)
    for s in [64, 128, 256, 512]:
        mark_sq.resize((s, s), Image.LANCZOS).save(
            out / f"mark-{s}.png", optimize=True, compress_level=9
        )

    # Horizontal lockup — covers wordmark surfaces (hero, footer).
    h_lock = Image.open(MASTER / "lockup-horizontal-color.png").convert("RGBA")
    hw, hh = h_lock.size
    for target_h in [120, 240, 480]:
        target_w = int(hw * target_h / hh)
        h_lock.resize((target_w, target_h), Image.LANCZOS).save(
            out / f"lockup-horizontal-{target_h}.png",
            optimize=True,
            compress_level=9,
        )

    # Vertical lockup — for centered hero/about usage if wanted.
    v_lock = Image.open(MASTER / "lockup-vertical-color.png").convert("RGBA")
    vw, vh = v_lock.size
    for target_h in [300, 600]:
        target_w = int(vw * target_h / vh)
        v_lock.resize((target_w, target_h), Image.LANCZOS).save(
            out / f"lockup-vertical-{target_h}.png",
            optimize=True,
            compress_level=9,
        )

    print(f"  → web: {len(list(out.glob('*')))} files")


def build_derived() -> None:
    print("\n[3/3] building derived")
    build_app_icons()
    build_tray()
    build_favicon()
    build_og()
    build_installer_win()
    build_social()
    build_web()


# ─── orchestration ─────────────────────────────────────────────────────────

def main() -> None:
    stage = sys.argv[1] if len(sys.argv) > 1 else "all"

    # `installer` rebuilds only the Windows installer art from the checked-in
    # masters — no source/ needed, and it won't churn unrelated derived assets.
    if stage == "installer":
        if not MASTER.exists():
            raise SystemExit(f"missing master/ — run a full build first ({MASTER})")
        print("rebuilding Windows installer art from master/")
        build_installer_win()
        print("\n✦ done.")
        return

    if not SOURCE.exists():
        raise SystemExit(f"missing source/ — copy raw PNGs to {SOURCE}")
    print(f"building hypercolor brand assets from {SOURCE}")
    build_masters()
    build_masks()
    build_derived()
    print("\n✦ done.")


if __name__ == "__main__":
    main()
