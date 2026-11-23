#!/usr/bin/env python3
"""Structural sample decks for the thumbnail dual-track gate
(LibreOffice retirement, route phase 2 / decision 3).

The historical pilot decks (Redwood — a real business deck — and Case2b)
are not in this repo by design (*.pptx is gitignored) and no longer exist
on any machine we control, so the CI gate renders:
  * Case2 (tools/make_case2.py): native charts x3, table, shapes,
    picture + animation timing + transition, embedded video;
  * the five decks generated here: text layout, shapes/fills, media +
    styled tables, a 4:3 deck, slide backgrounds + transparency.

Everything is synthetic and deterministic (images are drawn with PIL at
generation time; no external assets). Output dir: $QIMU_THUMB_SAMPLES_OUT
or argv[1] (default ./thumb-samples). Individual features that python-pptx
cannot express on the installed version are skipped with a warning, never
fatal — the deck still renders, the report lists what was skipped.

Usage: make_thumb_samples.py [outdir]
"""
import os, sys
from pptx import Presentation
from pptx.chart.data import ChartData
from pptx.dml.color import RGBColor
from pptx.enum.chart import XL_CHART_TYPE
from pptx.enum.shapes import MSO_SHAPE
from pptx.enum.text import PP_ALIGN
from pptx.oxml.ns import qn
from pptx.util import Inches, Pt

OUT = os.environ.get("QIMU_THUMB_SAMPLES_OUT") or (sys.argv[1] if len(sys.argv) > 1 else "./thumb-samples")
os.makedirs(OUT, exist_ok=True)
SKIPPED = []


def warn(what, e):
    SKIPPED.append(f"{what}: {e}")
    print(f"WARN skipped {what}: {e}", file=sys.stderr)


def set_font(run, name, size=None, bold=None, italic=None, color=None):
    """Set latin + east-asian typeface (CJK runs need the ea slot, or
    renderers fall back independently and the diff measures fonts, not
    rendering)."""
    f = run.font
    f.name = name
    if size is not None:
        f.size = Pt(size)
    if bold is not None:
        f.bold = bold
    if italic is not None:
        f.italic = italic
    if color is not None:
        f.color.rgb = RGBColor(*color)
    try:
        rPr = run._r.get_or_add_rPr()
        ea = rPr.find(qn("a:ea"))
        if ea is None:
            ea = rPr.makeelement(qn("a:ea"), {})
            rPr.append(ea)
        ea.set("typeface", name)
    except Exception as e:  # noqa: BLE001
        warn("ea typeface", e)


def textbox(slide, x, y, w, h, wrap=True):
    tx = slide.shapes.add_textbox(Inches(x), Inches(y), Inches(w), Inches(h))
    tx.text_frame.word_wrap = wrap
    return tx


def para(tf, text, first=False, **kw):
    align = kw.pop("align", None)
    p = tf.paragraphs[0] if first else tf.add_paragraph()
    r = p.add_run()
    r.text = text
    set_font(r, kw.pop("font", "Calibri"), **kw)
    if align is not None:
        p.alignment = align
    return p


def title_bar(slide, text):
    tx = textbox(slide, 0.5, 0.25, 12.3, 0.7)
    para(tx.text_frame, text, first=True, font="Microsoft YaHei", size=26, bold=True,
         color=(0x1F, 0x2A, 0x44))
    return tx


def blank(prs):
    return prs.slides.add_slide(prs.slide_layouts[6])


def shape(slide, name, x, y, w, h, fill=None, line=None, line_w=None, text=None):
    kind = getattr(MSO_SHAPE, name, None)
    if kind is None:
        warn(f"shape {name}", "not in MSO_SHAPE")
        return None
    sp = slide.shapes.add_shape(kind, Inches(x), Inches(y), Inches(w), Inches(h))
    if fill is None:
        sp.fill.background()
    else:
        sp.fill.solid()
        sp.fill.fore_color.rgb = RGBColor(*fill)
    if line is None:
        sp.line.fill.background()
    else:
        sp.line.color.rgb = RGBColor(*line)
        sp.line.width = Pt(line_w or 1.0)
    if text:
        sp.text_frame.text = text
        for p in sp.text_frame.paragraphs:
            for r in p.runs:
                set_font(r, "Microsoft YaHei", size=12, color=(0xFF, 0xFF, 0xFF))
    try:
        sp.shadow.inherit = False
    except Exception:  # noqa: BLE001 - shadow control is cosmetic here
        pass
    return sp


def set_fill_alpha(sp, opacity_pct):
    """Inject a:alpha into the solidFill (python-pptx has no API)."""
    try:
        sF = sp.fill._xPr.find(qn("a:solidFill"))
        clr = sF.find(qn("a:srgbClr"))
        el = clr.makeelement(qn("a:alpha"), {"val": str(int(opacity_pct * 1000))})
        clr.append(el)
    except Exception as e:  # noqa: BLE001
        warn("fill alpha", e)


def make_photo(path, w=960, h=540, base=(30, 70, 120)):
    from PIL import Image, ImageDraw
    img = Image.new("RGB", (w, h), base)
    d = ImageDraw.Draw(img)
    for i in range(h):
        d.line([(0, i), (w, i)], fill=(base[0] + i // 9, base[1] + i // 7, base[2] + i // 6))
    d.rectangle([80, 300, 620, 470], fill=(232, 139, 46))
    d.ellipse([640, 90, 880, 330], fill=(245, 197, 66))
    d.rectangle([120, 120, 420, 240], outline=(255, 255, 255), width=6)
    img.save(path)
    return path


# ---------------------------------------------------------------- deck 1
def deck_text():
    prs = Presentation()
    prs.slide_width, prs.slide_height = Inches(13.333), Inches(7.5)
    # s1: title + subtitle, CJK + Latin, sizes
    s = blank(prs)
    tx = textbox(s, 0.7, 1.4, 12.0, 1.2)
    para(tx.text_frame, "启幕缩略图双轨样本 · 文本版式", first=True,
         font="Microsoft YaHei", size=40, bold=True, color=(0x1F, 0x6F, 0xB2))
    tx = textbox(s, 0.7, 2.8, 12.0, 0.8)
    para(tx.text_frame, "Text layout sample — mixed CJK / Latin, sizes 12–40pt",
         first=True, size=20, color=(0x55, 0x55, 0x55))
    tx = textbox(s, 0.7, 4.2, 12.0, 1.6)
    para(tx.text_frame, "小字号正文 12pt：电池回收产线巡检记录，苏州工厂一号线运行正常。",
         first=True, font="Microsoft YaHei", size=12)
    para(tx.text_frame, "Italic Latin 14pt with numbers 0123456789 and symbols ±×÷.",
         size=14, italic=True)
    # s2: dense body, bullets, alignment
    s = blank(prs)
    title_bar(s, "多段正文与项目符号")
    tx = textbox(s, 0.6, 1.2, 7.6, 5.6)
    para(tx.text_frame, "一级条目：来料检验", first=True, font="Microsoft YaHei", size=16, bold=True)
    p = para(tx.text_frame, "二级条目：电压与内阻抽测，每批 32 只。", font="Microsoft YaHei", size=14)
    p.level = 1
    p = para(tx.text_frame, "三级条目：异常批次隔离并复检，记录留存三年。", font="Microsoft YaHei", size=12)
    p.level = 2
    para(tx.text_frame, "普通段落：正极材料占比 38%，负极材料 22%，电解液 15%，隔膜 10%，其他 15%。",
         font="Microsoft YaHei", size=13)
    para(tx.text_frame, "红色提示 13pt：带电量高的三元电池须单独存放。", font="Microsoft YaHei",
         size=13, color=(0xC0, 0x39, 0x2B))
    tx = textbox(s, 8.8, 1.2, 3.9, 5.6)
    para(tx.text_frame, "居中对齐", first=True, font="Microsoft YaHei", size=14, align=PP_ALIGN.CENTER)
    para(tx.text_frame, "右对齐 Right", font="Microsoft YaHei", size=14, align=PP_ALIGN.RIGHT)
    para(tx.text_frame, "左对齐 Left", font="Microsoft YaHei", size=14, align=PP_ALIGN.LEFT)
    # s3: filled / rotated text boxes
    s = blank(prs)
    title_bar(s, "填充文本框与旋转")
    tx = textbox(s, 0.6, 1.3, 5.6, 1.6)
    try:
        tx.fill.solid()
        tx.fill.fore_color.rgb = RGBColor(0x1F, 0x6F, 0xB2)
        tx.line.color.rgb = RGBColor(0x12, 0x3A, 0x5E)
        tx.line.width = Pt(2.25)
        fill_note = ""
    except Exception as e:  # noqa: BLE001 - older python-pptx TextBox lacks fill
        warn("textbox fill", e)
        fill_note = "（填充不可用，已降级）"
    para(tx.text_frame, f"蓝底白字文本框（填充 + 描边）{fill_note}", first=True,
         font="Microsoft YaHei", size=16, bold=True, color=(0xFF, 0xFF, 0xFF))
    tx = textbox(s, 7.2, 1.5, 4.6, 1.2)
    tx.rotation = 12
    para(tx.text_frame, "旋转 12° 的文本框 Rotated", first=True,
         font="Microsoft YaHei", size=15, color=(0x8E, 0x44, 0x1D))
    tx = textbox(s, 0.6, 3.8, 12.0, 1.0, wrap=False)
    para(tx.text_frame, "不换行长文本：苏州—常州—宜春三地工厂产能协同排产与物流调度统一纳入季度计划评审。",
         first=True, font="Microsoft YaHei", size=14)
    prs.save(f"{OUT}/Text_Layout.pptx")
    print("Text_Layout.pptx")


# ---------------------------------------------------------------- deck 2
def deck_shapes():
    prs = Presentation()
    prs.slide_width, prs.slide_height = Inches(13.333), Inches(7.5)
    s = blank(prs)
    title_bar(s, "自选图形与填充")
    names = ["RECTANGLE", "ROUNDED_RECTANGLE", "OVAL", "ISOCELES_TRIANGLE",
             "RIGHT_ARROW", "CHEVRON", "PENTAGON", "HEXAGON", "DIAMOND",
             "PARALLELOGRAM", "DONUT", "HEART", "SUN", "CLOUD", "STAR_5_POINT",
             "LIGHTNING_BOLT"]
    fills = [(0x1F, 0x6F, 0xB2), (0xE8, 0x8B, 0x2E), (0x27, 0xAE, 0x60),
             (0x8E, 0x44, 0xAD), (0xC0, 0x39, 0x2B), (0x16, 0xA0, 0x85)]
    for i, n in enumerate(names):
        x = 0.6 + (i % 6) * 2.1
        y = 1.3 + (i // 6) * 1.9
        shape(s, n, x, y, 1.7, 1.3, fill=fills[i % len(fills)])
    s = blank(prs)
    title_bar(s, "描边、虚线与无填充")
    shape(s, "RECTANGLE", 0.6, 1.4, 3.2, 1.8, fill=None, line=(0x1F, 0x6F, 0xB2), line_w=3)
    sp = shape(s, "ROUNDED_RECTANGLE", 4.4, 1.4, 3.2, 1.8, fill=(0xFD, 0xED, 0xEC),
               line=(0xC0, 0x39, 0x2B), line_w=1.5)
    try:
        from pptx.enum.line import MSO_LINE
        sp.line.dash_style = MSO_LINE.DASH
    except Exception as e:  # noqa: BLE001
        warn("dash style", e)
    shape(s, "OVAL", 8.2, 1.4, 2.2, 1.8, fill=(0xFE, 0xF9, 0xE7), line=(0xB7, 0x95, 0x0B), line_w=6)
    try:
        from pptx.enum.shapes import MSO_CONNECTOR
        cn = s.shapes.add_connector(MSO_CONNECTOR.STRAIGHT, Inches(0.6), Inches(4.2),
                                    Inches(5.4), Inches(5.6))
        cn.line.color.rgb = RGBColor(0x1F, 0x6F, 0xB2)
        cn.line.width = Pt(2.5)
        fb = s.shapes.build_freeform(Inches(7.0), Inches(4.0), scale=1.0)
        fb.add_line_segments([(Inches(8.6), Inches(5.8)), (Inches(10.4), Inches(4.4)),
                              (Inches(11.8), Inches(6.0))], close=True)
        fs = fb.convert_to_shape()
        fs.fill.solid()
        fs.fill.fore_color.rgb = RGBColor(0x2E, 0x86, 0xC1)
    except Exception as e:  # noqa: BLE001
        warn("connector/freeform", e)
    s = blank(prs)
    title_bar(s, "半透明覆盖与组合")
    photo = make_photo("/tmp/thumb-sample-photo2.png", base=(90, 60, 30))
    s.shapes.add_picture(photo, Inches(0.6), Inches(1.3), Inches(7.6), Inches(4.3))
    ov = shape(s, "RECTANGLE", 0.6, 4.3, 7.6, 1.3, fill=(0x1F, 0x6F, 0xB2))
    if ov is not None:
        set_fill_alpha(ov, 55)
    try:
        a = shape(s, "OVAL", 9.2, 1.6, 1.6, 1.6, fill=(0xE8, 0x8B, 0x2E))
        b = shape(s, "RECTANGLE", 10.2, 2.6, 2.0, 1.4, fill=(0x27, 0xAE, 0x60))
        s.shapes.add_group_shape([a, b], Inches(9.0), Inches(1.4), Inches(3.4), Inches(3.4))
    except Exception as e:  # noqa: BLE001
        warn("group shape", e)
    prs.save(f"{OUT}/Shapes_Fills.pptx")
    print("Shapes_Fills.pptx")


# ---------------------------------------------------------------- deck 3
def deck_media():
    prs = Presentation()
    prs.slide_width, prs.slide_height = Inches(13.333), Inches(7.5)
    photo = make_photo("/tmp/thumb-sample-photo.png")
    s = blank(prs)
    title_bar(s, "图片：裁剪、旋转、描边")
    s.shapes.add_picture(photo, Inches(0.6), Inches(1.3), Inches(5.8), Inches(3.3))
    pic = s.shapes.add_picture(photo, Inches(7.0), Inches(1.3), Inches(5.4), Inches(3.3))
    pic.crop_left = 0.18
    pic.crop_right = 0.12
    pic.line.color.rgb = RGBColor(0x1F, 0x6F, 0xB2)
    pic.line.width = Pt(3)
    pic2 = s.shapes.add_picture(photo, Inches(4.6), Inches(4.9), Inches(3.6), Inches(2.0))
    pic2.rotation = 8
    s = blank(prs)
    title_bar(s, "样式表格（表头填充 / 隔行色 / 合并单元格）")
    tbl_shape = s.shapes.add_table(5, 4, Inches(0.6), Inches(1.4), Inches(12.1), Inches(3.6))
    tbl = tbl_shape.table
    data = [["工厂", "产能（t/d)", "良品率", "状态"],
            ["苏州", "120", "98.2%", "在产"],
            ["常州", "80", "97.6%", "在产"],
            ["宜春", "60", "—", "建设中"],
            ["合计", "260", "—", ""]]
    for r in range(5):
        for c in range(4):
            cell = tbl.cell(r, c)
            cell.text = data[r][c]
            for p in cell.text_frame.paragraphs:
                for run in p.runs:
                    set_font(run, "Microsoft YaHei", size=13,
                             bold=(r == 0),
                             color=(0xFF, 0xFF, 0xFF) if r == 0 else (0x1B, 0x1B, 0x1B))
            cell.fill.solid()
            if r == 0:
                cell.fill.fore_color.rgb = RGBColor(0x1F, 0x6F, 0xB2)
            elif r % 2 == 0:
                cell.fill.fore_color.rgb = RGBColor(0xD6, 0xE9, 0xF8)
            else:
                cell.fill.fore_color.rgb = RGBColor(0xFF, 0xFF, 0xFF)
    try:
        tbl.cell(4, 2).merge(tbl.cell(4, 3))
    except Exception as e:  # noqa: BLE001
        warn("cell merge", e)
    s = blank(prs)
    s.shapes.add_picture(photo, Inches(0), Inches(0), Inches(13.333), Inches(7.5))
    tx = textbox(s, 0.8, 2.9, 11.7, 1.4)
    para(tx.text_frame, "全幅底图 + 标题覆盖", first=True, font="Microsoft YaHei",
         size=36, bold=True, color=(0xFF, 0xFF, 0xFF))
    prs.save(f"{OUT}/Media_Table.pptx")
    print("Media_Table.pptx")


# ---------------------------------------------------------------- deck 4
def deck_43():
    prs = Presentation()
    prs.slide_width, prs.slide_height = Inches(10.0), Inches(7.5)  # 4:3
    s = blank(prs)
    tx = textbox(s, 0.5, 0.3, 9.0, 0.8)
    para(tx.text_frame, "4:3 画幅混合页", first=True, font="Microsoft YaHei",
         size=28, bold=True, color=(0x1F, 0x2A, 0x44))
    cd = ChartData()
    cd.categories = ["1月", "2月", "3月", "4月"]
    cd.add_series("回收量（吨）", (42, 55, 61, 58))
    s.shapes.add_chart(XL_CHART_TYPE.LINE_MARKERS, Inches(0.5), Inches(1.3),
                       Inches(5.8), Inches(4.6), cd)
    shape(s, "ROUNDED_RECTANGLE", 6.8, 1.5, 2.7, 1.3, fill=(0x1F, 0x6F, 0xB2),
          text="重点：宜春厂")
    shape(s, "RIGHT_ARROW", 7.3, 3.4, 1.8, 0.8, fill=(0xE8, 0x8B, 0x2E))
    tx = textbox(s, 6.8, 4.6, 2.9, 1.6)
    para(tx.text_frame, "4:3 旧模板仍在部分工厂使用，缩略图须等比正确。",
         first=True, font="Microsoft YaHei", size=12)
    s = blank(prs)
    tx = textbox(s, 0.5, 0.3, 9.0, 0.8)
    para(tx.text_frame, "4:3 纯文本页", first=True, font="Microsoft YaHei",
         size=24, bold=True)
    tx = textbox(s, 0.5, 1.4, 9.0, 5.0)
    para(tx.text_frame, "第一段：4:3 页面上的中文长段落，检验换行与行距在两种渲染器下的一致性。" * 2,
         first=True, font="Microsoft YaHei", size=14)
    para(tx.text_frame, "Second paragraph in Latin with numbers 1234567890.", size=14)
    prs.save(f"{OUT}/Deck_43.pptx")
    print("Deck_43.pptx")


# ---------------------------------------------------------------- deck 5
def deck_bg():
    prs = Presentation()
    prs.slide_width, prs.slide_height = Inches(13.333), Inches(7.5)
    s = blank(prs)
    try:
        s.background.fill.solid()
        s.background.fill.fore_color.rgb = RGBColor(0x14, 0x22, 0x38)
    except Exception as e:  # noqa: BLE001
        warn("slide background", e)
        shape(s, "RECTANGLE", 0, 0, 13.333, 7.5, fill=(0x14, 0x22, 0x38))
    tx = textbox(s, 0.8, 2.6, 11.7, 1.2)
    para(tx.text_frame, "深色底 + 浅色字", first=True, font="Microsoft YaHei",
         size=36, bold=True, color=(0xEC, 0xF0, 0xF1))
    tx = textbox(s, 0.8, 4.0, 11.7, 0.8)
    para(tx.text_frame, "Dark slide background with light text — contrast check.",
         first=True, size=16, color=(0xAB, 0xBD, 0xC6))
    s = blank(prs)
    title_bar(s, "色带与近渐变（双层半透明）")
    band = shape(s, "RECTANGLE", 0.6, 1.4, 12.1, 2.2, fill=(0x1F, 0x6F, 0xB2))
    band2 = shape(s, "RECTANGLE", 0.6, 2.5, 12.1, 1.1, fill=(0x5D, 0xA6, 0xE8))
    if band2 is not None:
        set_fill_alpha(band2, 60)
    tx = textbox(s, 0.9, 1.8, 11.0, 0.9)
    para(tx.text_frame, "品牌色带 Banner Band", first=True, font="Microsoft YaHei",
         size=24, bold=True, color=(0xFF, 0xFF, 0xFF))
    shape(s, "RECTANGLE", 0.6, 4.4, 3.8, 1.6, fill=(0x27, 0xAE, 0x60))
    shape(s, "RECTANGLE", 4.8, 4.4, 3.8, 1.6, fill=(0xE8, 0x8B, 0x2E))
    shape(s, "RECTANGLE", 9.0, 4.4, 3.8, 1.6, fill=(0x8E, 0x44, 0xAD))
    prs.save(f"{OUT}/Backgrounds.pptx")
    print("Backgrounds.pptx")


if __name__ == "__main__":
    deck_text()
    deck_shapes()
    deck_media()
    deck_43()
    deck_bg()
    if SKIPPED:
        print("skipped features:", "; ".join(SKIPPED), file=sys.stderr)
    print(f"samples -> {OUT}")
