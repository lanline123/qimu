#!/usr/bin/env python3
"""Verdict gate for the thumbnail dual-track comparison
(LibreOffice retirement, route phase 2 / decision 3).

Reads the report.json produced by tools/pilot_deckcraft_render.py and
applies the retirement gates. Writes verdict.json + verdict.md into the
output dir and exits 0 (PASS) / 1 (FAIL) so the CI job goes red when
DeckCraft is not yet good enough to be the only renderer.

Gates (hard):
  * completeness — every deck: no DeckCraft error, pages_dc == pages_lo,
    pages_lo >= 1;
  * per-deck mean_abs_diff_avg <= 6.0;
  * per-page  mean_abs_diff     <= 12.0.
Calibration: the original pilot (author's machine, real Redwood deck +
Case2 + Case2b) measured per-deck averages <= 4.8 and a worst page of
8.2 (a chart page). The gates add ~25% / ~45% headroom for the CI font
environment (Carlito + Noto CJK substitutes vs the decks' named fonts).
If a first CI run fails ONLY on text-dense pages with low hot_frac, the
right move is to recalibrate with the run's numbers recorded here — not
to loosen silently.

Warnings (reported, not failing):
  * any page hot_frac > 0.15, or a deck hot_frac average > 0.05 —
    a large hot area means a whole region renders differently
    (missing shape/image class), which mean-diff alone can hide.

Scope caveat (also printed into verdict.md): every sample in the gate
is synthetic (Case2 + tools/make_thumb_samples.py). The real business
decks used by the original pilot are not in this repo by design, so a
PASS means "gate passed on the structural sample set", not "proven on
the organisation's real decks".

Usage: thumb_verdict.py <report.json> <outdir>
"""
import json, os, sys

DECK_AVG_MAX = 6.0
PAGE_MAX = 12.0
HOT_PAGE_WARN = 0.15
HOT_DECK_WARN = 0.05


def main():
    report_path = sys.argv[1] if len(sys.argv) > 1 else "/tmp/deckcraft-pilot/report.json"
    outdir = sys.argv[2] if len(sys.argv) > 2 else "verdict-out"
    os.makedirs(outdir, exist_ok=True)
    with open(report_path) as f:
        report = json.load(f)

    failures, warnings, rows = [], [], []
    total_pages = 0
    for tag in sorted(report):
        d = report[tag]
        pages = d.get("pages", [])
        total_pages += len(pages)
        diffs = [p["mean_abs_diff"] for p in pages if p["mean_abs_diff"] is not None]
        hots = [p["hot_frac"] for p in pages if p["hot_frac"] is not None]
        worst = max(diffs) if diffs else None
        hot_avg = round(sum(hots) / len(hots), 4) if hots else None
        rows.append({
            "deck": tag, "pages_lo": d.get("pages_lo"), "pages_dc": d.get("pages_dc"),
            "avg": d.get("mean_abs_diff_avg"), "worst_page": worst,
            "hot_avg": hot_avg,
            "lo_s": d.get("lo_seconds"), "dc_s": d.get("dc_seconds"),
            "dc_error": d.get("dc_error"),
        })
        if d.get("dc_error"):
            failures.append(f"{tag}: DeckCraft 渲染报错: {d['dc_error']}")
        if not d.get("pages_lo"):
            failures.append(f"{tag}: LibreOffice 未产出页面（样本或 LO 链异常）")
        elif d.get("pages_dc") != d.get("pages_lo"):
            failures.append(f"{tag}: 页数不一致 LO={d.get('pages_lo')} DC={d.get('pages_dc')}")
        if len(diffs) != len(pages):
            failures.append(f"{tag}: {len(pages) - len(diffs)} 页无可比图像")
        avg = d.get("mean_abs_diff_avg")
        if avg is not None and avg > DECK_AVG_MAX:
            failures.append(f"{tag}: 整稿平均像素差 {avg} > 门槛 {DECK_AVG_MAX}")
        for p in pages:
            m = p["mean_abs_diff"]
            if m is not None and m > PAGE_MAX:
                failures.append(f"{tag} 第 {p['page']} 页: 像素差 {m} > 门槛 {PAGE_MAX}")
            h = p["hot_frac"]
            if h is not None and h > HOT_PAGE_WARN:
                warnings.append(f"{tag} 第 {p['page']} 页: 大差异像素占比 {h} > {HOT_PAGE_WARN}（整区渲染差异，人工看图）")
        if hot_avg is not None and hot_avg > HOT_DECK_WARN:
            warnings.append(f"{tag}: 整稿大差异像素占比均值 {hot_avg} > {HOT_DECK_WARN}")

    verdict = {
        "pass": not failures,
        "decks": len(report), "pages": total_pages,
        "gates": {"deck_avg_max": DECK_AVG_MAX, "page_max": PAGE_MAX,
                  "hot_page_warn": HOT_PAGE_WARN, "hot_deck_warn": HOT_DECK_WARN},
        "rows": rows, "failures": failures, "warnings": warnings,
        "scope": "synthetic samples only (Case2 + make_thumb_samples); real business decks not in repo",
    }
    with open(os.path.join(outdir, "verdict.json"), "w") as f:
        json.dump(verdict, f, ensure_ascii=False, indent=1)

    lines = [
        f"# 缩略图双轨判定：{'PASS — 达标' if verdict['pass'] else 'FAIL — 未达标'}",
        "",
        f"样本：{len(report)} 稿 / {total_pages} 页（全部为合成样本：Case2 + make_thumb_samples 结构样本；"
        "真实业务稿不在仓内，见报告说明）。",
        f"门槛：整稿平均 ≤ {DECK_AVG_MAX}，单页 ≤ {PAGE_MAX}（标定依据：原试点整稿 ≤ 4.8、最差页 8.2，"
        "加 CI 字体环境余量）。",
        "",
        "| 稿 | 页 LO/DC | 整稿平均差 | 最差页 | 大差异占比均值 | LO 秒 | DC 秒 |",
        "|---|---|---|---|---|---|---|",
    ]
    for r in rows:
        lines.append(f"| {r['deck']} | {r['pages_lo']}/{r['pages_dc']} | {r['avg']} | "
                     f"{r['worst_page']} | {r['hot_avg']} | {r['lo_s']} | {r['dc_s']} |")
    if failures:
        lines += ["", "## 未达标项"] + [f"- {x}" for x in failures]
    if warnings:
        lines += ["", "## 警告（不判红，须人工看图）"] + [f"- {x}" for x in warnings]
    if verdict["pass"]:
        lines += ["", "按路线阶段 2 决策③：双轨达标，LibreOffice 可退役（改动清单见 qimu-thumb-retire 报告 §6）。"]
    with open(os.path.join(outdir, "verdict.md"), "w") as f:
        f.write("\n".join(lines) + "\n")
    print("\n".join(lines))
    sys.exit(0 if verdict["pass"] else 1)


if __name__ == "__main__":
    main()
