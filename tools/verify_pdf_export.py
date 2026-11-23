#!/usr/bin/env python3
"""Q21 hard gate: DeckCraft PDF export — text-layer evidence (CI side).

Runs the EXACT invocation qimu-server's deck_to_pdf uses
(`deckcraft-cli run --in deck.pptx --export out.pdf --export-params ...`)
against a real-structure sample deck (CI generates it with
tools/make_case2.py, same as the p0 workflow) and proves, from the
produced PDF's own bytes:

  1. the export succeeds and yields a %PDF with the expected pages;
  2. page 1 carries REAL text: extractable text containing the deck's
     known slide-1 string, and text-showing operators (Tj/TJ) in the
     page content stream — i.e. text is selectable, not only pixels;
  3. the handout and notes forms export successfully too.

It also CHARACTERISES the form honestly (printed, and the notes form
is checked for the slide's speaker-notes string): deckcraft-pdf
rasterises the slide artwork (default 200 dpi) and lays an invisible
real-text layer over it, so the expected verdict is HYBRID — raster
artwork + selectable text — NOT a pure vector PDF. If a future
DeckCraft changes that model, this script's PDF_FORM line changes
with the evidence, not with anyone's say-so.

Usage: verify_pdf_export.py <deckcraft-cli> <deck.pptx>
Requires: pip install pypdf  (CI step; no other deps)
Exit: 0 = gate pass, 1 = gate fail, 2 = environment problem.
"""
import json
import os
import subprocess
import sys
import tempfile

CLI = sys.argv[1] if len(sys.argv) > 1 else "deckcraft-cli"
DECK = sys.argv[2] if len(sys.argv) > 2 else ""

# Known strings from tools/make_case2.py (slide 1).
SLIDE1_ASCII = "Scenario-13"                    # subtitle (hard anchor)
SLIDE1_CJK = "图表与动画保真测试"                  # title (informational)
NOTES_CJK = "开场备注"                            # slide-1 speaker notes

try:
    from pypdf import PdfReader
except ImportError:
    try:
        from PyPDF2 import PdfReader
    except ImportError:
        print("ENV FAIL: pypdf not installed (pip install pypdf)")
        sys.exit(2)

if not DECK or not os.path.exists(DECK):
    print(f"ENV FAIL: deck not found: {DECK!r}")
    sys.exit(2)


def export(tag, params):
    out = os.path.join(tempfile.mkdtemp(prefix="qimu-pdf-"), f"{tag}.pdf")
    r = subprocess.run(
        [CLI, "run", "--in", DECK, "--export", out,
         "--export-params", json.dumps(params)],
        capture_output=True, text=True, timeout=600)
    if r.returncode != 0 or not os.path.exists(out):
        print(f"FAIL[{tag}]: deckcraft-cli exit={r.returncode}\n{r.stdout}\n{r.stderr}")
        sys.exit(1)
    with open(out, "rb") as fh:
        magic = fh.read(5)
    if magic != b"%PDF-":
        print(f"FAIL[{tag}]: output does not start with %PDF-")
        sys.exit(1)
    return out


def page_text(reader, idx):
    try:
        return reader.pages[idx].extract_text() or ""
    except Exception as e:  # noqa: BLE001 - evidence script reports, not crashes
        print(f"  (extract_text page {idx} raised: {e})")
        return ""


def content_stream(reader, idx):
    try:
        return reader.pages[idx].get_contents().get_data()
    except Exception as e:  # noqa: BLE001
        print(f"  (content stream page {idx} unreadable: {e})")
        return b""


def image_xobjects(reader, idx):
    try:
        res = reader.pages[idx]["/Resources"]
        xo = res.get("/XObject")
        if not xo:
            return 0
        return sum(1 for k in xo.keys()
                   if xo[k].get_object().get("/Subtype") == "/Image")
    except Exception:  # noqa: BLE001
        return -1


def main():
    failures = []

    # --- slides form, page 1 only (the server's single-page variant) ---
    pdf1 = export("slides-p1", {"layout": "slides", "textLayer": True, "slides": [0]})
    r1 = PdfReader(pdf1)
    n1 = len(r1.pages)
    print(f"slides[0]: pages={n1}")
    if n1 != 1:
        failures.append(f"single-page export produced {n1} pages, expected 1")
    text1 = page_text(r1, 0)
    print(f"slides[0]: extracted {len(text1)} chars; ascii anchor "
          f"{SLIDE1_ASCII!r} present: {SLIDE1_ASCII in text1}; "
          f"CJK title present: {SLIDE1_CJK in text1}")
    if SLIDE1_ASCII not in text1:
        failures.append("slide-1 text NOT extractable (no selectable text layer?)")
    stream = content_stream(r1, 0)
    has_ops = (b" Tj" in stream) or (b" TJ" in stream) or stream.rstrip().endswith(b"Tj")
    print(f"slides[0]: content stream {len(stream)} B; text-showing ops (Tj/TJ): {has_ops}")
    if not has_ops:
        failures.append("no Tj/TJ operators in page content stream")
    imgs = image_xobjects(r1, 0)
    print(f"slides[0]: image XObjects on page: {imgs}")

    # --- handout form (whole deck) ---
    pdfh = export("handout", {"layout": "handouts", "perPage": 6, "textLayer": True})
    rh = PdfReader(pdfh)
    print(f"handout: pages={len(rh.pages)}")
    if len(rh.pages) < 1:
        failures.append("handout export produced no pages")

    # --- notes form (whole deck; notes text must show up) ---
    pdfn = export("notes", {"layout": "notes", "textLayer": True})
    rn = PdfReader(pdfn)
    all_notes_text = "\n".join(page_text(rn, i) for i in range(len(rn.pages)))
    print(f"notes: pages={len(rn.pages)}; slide title present: "
          f"{SLIDE1_ASCII in all_notes_text}; speaker-notes string "
          f"{NOTES_CJK!r} present: {NOTES_CJK in all_notes_text}")
    if len(rn.pages) < 1:
        failures.append("notes export produced no pages")
    if SLIDE1_ASCII not in all_notes_text:
        failures.append("notes form lost the slide text")

    # --- honest form verdict ---
    if imgs and imgs > 0:
        print("PDF_FORM: HYBRID — raster slide artwork (image XObject) "
              "+ real selectable text layer; NOT a pure vector PDF")
    elif imgs == 0:
        print("PDF_FORM: NO-RASTER — no image XObject on page 1; "
              "artwork appears vector-drawn, text layer real")
    else:
        print("PDF_FORM: UNKNOWN — image XObject count unreadable")

    if failures:
        for f in failures:
            print("GATE FAIL:", f)
        sys.exit(1)
    print("GATE: PASS")


if __name__ == "__main__":
    main()
