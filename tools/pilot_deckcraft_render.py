#!/usr/bin/env python3
"""Step-3 pilot: DeckCraft render vs the server's LibreOffice thumbnail
pipeline, page by page (component-integration ledger, design/组件接入台账.md).

For each input deck:
  * LO path  — identical to qimu-server's thumb renderer: soffice
    --headless --convert-to pdf, then pdftoppm -png -r 55.
  * DC path  — deckcraft-cli render --all --scale 1.0, resized to the LO
    page size for comparison.
Metrics per page: mean absolute pixel difference (0..255) and the share
of pixels whose largest channel difference exceeds 32. Plus wall time.

Usage: pilot_deckcraft_render.py <deckcraft-cli> <deck.pptx> [...]
Writes PNGs + report.json under /tmp/deckcraft-pilot/<deck-tag>/.
"""
import glob, json, os, subprocess, sys, time
from PIL import Image, ImageChops

DC = sys.argv[1]
DECKS = sys.argv[2:]
ROOT = "/tmp/deckcraft-pilot"


def render_lo(pptx, d):
    os.makedirs(d, exist_ok=True)
    subprocess.run(["cp", pptx, f"{d}/in.pptx"], check=True)
    t0 = time.time()
    subprocess.run(
        ["soffice", f"-env:UserInstallation=file://{d}/lo", "--headless",
         "--convert-to", "pdf", "--outdir", d, f"{d}/in.pptx"],
        capture_output=True, timeout=600)
    subprocess.run(["pdftoppm", "-png", "-r", "55", f"{d}/in.pdf", f"{d}/lo"],
                   capture_output=True, timeout=300)
    return time.time() - t0, sorted(glob.glob(f"{d}/lo-*.png"))


def render_dc(pptx, d):
    out = f"{d}/dc"
    os.makedirs(out, exist_ok=True)
    t0 = time.time()
    r = subprocess.run([DC, "render", pptx, "--all", out, "--scale", "1.0"],
                       capture_output=True, text=True, timeout=600)
    dt = time.time() - t0
    if r.returncode != 0:
        raise RuntimeError(f"deckcraft render failed: {r.stdout} {r.stderr}")
    return dt, sorted(glob.glob(f"{out}/slide-*.png"))


def page_metrics(lo_png, dc_png):
    ia = Image.open(lo_png).convert("RGB")
    ib = Image.open(dc_png).convert("RGB")
    if ib.size != ia.size:
        ib = ib.resize(ia.size, Image.LANCZOS)
    diff = ImageChops.difference(ia, ib)
    px = list(diff.getdata())
    n = len(px)
    mean_abs = sum(sum(p) for p in px) / (n * 3)
    hot = sum(1 for p in px if max(p) > 32) / n
    return round(mean_abs, 2), round(hot, 4)


def main():
    report = {}
    for deck in DECKS:
        tag = os.path.splitext(os.path.basename(deck))[0]
        d = f"{ROOT}/{tag}"
        subprocess.run(["rm", "-rf", d])
        lo_t, lo_pngs = render_lo(deck, d)
        try:
            dc_t, dc_pngs = render_dc(deck, d)
            err = None
        except Exception as e:  # noqa: BLE001 - pilot records, not crashes
            dc_t, dc_pngs, err = 0.0, [], str(e)
        pages = []
        for i, lp in enumerate(lo_pngs):
            if i < len(dc_pngs):
                m, h = page_metrics(lp, dc_pngs[i])
                pages.append({"page": i + 1, "mean_abs_diff": m, "hot_frac": h})
            else:
                pages.append({"page": i + 1, "mean_abs_diff": None, "hot_frac": None})
        report[tag] = {
            "deck": deck, "pages_lo": len(lo_pngs), "pages_dc": len(dc_pngs),
            "lo_seconds": round(lo_t, 1), "dc_seconds": round(dc_t, 1),
            "dc_error": err, "pages": pages,
            "mean_abs_diff_avg": (round(sum(p["mean_abs_diff"] for p in pages
                                            if p["mean_abs_diff"] is not None)
                                        / max(1, sum(1 for p in pages
                                                     if p["mean_abs_diff"] is not None)), 2)
                                  if pages else None),
            "mean_abs_diff_max": max((p["mean_abs_diff"] for p in pages
                                      if p["mean_abs_diff"] is not None), default=None),
        }
        print(f"{tag}: LO {len(lo_pngs)}p in {lo_t:.1f}s | DC {len(dc_pngs)}p "
              f"in {dc_t:.1f}s | avg diff {report[tag]['mean_abs_diff_avg']} "
              f"max {report[tag]['mean_abs_diff_max']} err={err}")
    os.makedirs(ROOT, exist_ok=True)
    with open(f"{ROOT}/report.json", "w") as f:
        json.dump(report, f, ensure_ascii=False, indent=1)
    print(f"report -> {ROOT}/report.json")


if __name__ == "__main__":
    main()
