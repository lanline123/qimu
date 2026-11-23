#!/usr/bin/env python3
"""Generate case deck #2 for the scenario-13 fidelity gate:
native charts (bar/line/pie), a table, shapes, a picture with an entrance
animation (hand-injected p:timing), a slide transition, notes on most
slides, and an embedded video if ffmpeg is available."""
import os, subprocess, sys
from pptx import Presentation
from pptx.chart.data import ChartData
from pptx.enum.chart import XL_CHART_TYPE
from pptx.util import Inches, Pt, Emu
from pptx.dml.color import RGBColor
from lxml import etree

OUT = os.environ.get("QIMU_CASE2_OUT", "/home/hatch/workspace/user/files/Case2_Charts_Animation.pptx")
prs = Presentation()
prs.slide_width = Inches(13.333)
prs.slide_height = Inches(7.5)
blank = prs.slide_layouts[6]
title_layout = prs.slide_layouts[0]

def notes(slide, text):
    slide.notes_slide.notes_text_frame.text = text

def add_title(slide, text):
    tx = slide.shapes.add_textbox(Inches(0.5), Inches(0.25), Inches(12), Inches(0.7))
    tx.text_frame.text = text
    tx.text_frame.paragraphs[0].runs[0].font.size = Pt(28)
    tx.text_frame.paragraphs[0].runs[0].font.bold = True
    return tx

# 1 — title
s = prs.slides.add_slide(title_layout)
s.shapes.title.text = "案例 2 · 图表与动画保真测试"
s.placeholders[1].text = "Scenario-13 fidelity case deck #2"
notes(s, "开场备注：本稿用于验证图表、动画、备注在拆分/组装/原地替换后的保真。")

# 2 — bar chart
s = prs.slides.add_slide(blank)
add_title(s, "季度销量（柱状图）")
cd = ChartData()
cd.categories = ["Q1", "Q2", "Q3", "Q4"]
cd.add_series("华东", (120, 150, 170, 210))
cd.add_series("华北", (90, 110, 105, 140))
s.shapes.add_chart(XL_CHART_TYPE.COLUMN_CLUSTERED, Inches(0.6), Inches(1.2),
                   Inches(8.5), Inches(5.2), cd)
notes(s, "柱状图备注：华东 Q4 冲到 210。")

# 3 — line chart
s = prs.slides.add_slide(blank)
add_title(s, "回收量趋势（折线图）")
cd = ChartData()
cd.categories = ["1月", "2月", "3月", "4月", "5月", "6月"]
cd.add_series("回收量（吨）", (42, 55, 61, 58, 74, 90))
s.shapes.add_chart(XL_CHART_TYPE.LINE_MARKERS, Inches(0.6), Inches(1.2),
                   Inches(8.5), Inches(5.2), cd)
notes(s, "折线图备注：6 月达 90 吨。")

# 4 — pie chart
s = prs.slides.add_slide(blank)
add_title(s, "材料构成（饼图）")
cd = ChartData()
cd.categories = ["正极材料", "负极材料", "电解液", "隔膜", "其他"]
cd.add_series("占比", (0.38, 0.22, 0.15, 0.10, 0.15))
s.shapes.add_chart(XL_CHART_TYPE.PIE, Inches(0.6), Inches(1.2),
                   Inches(7.5), Inches(5.2), cd)
notes(s, "饼图备注：正极材料占比最高。")

# 5 — table + shapes
s = prs.slides.add_slide(blank)
add_title(s, "产能表与形状")
rows, cols = 4, 3
tbl_shape = s.shapes.add_table(rows, cols, Inches(0.6), Inches(1.3), Inches(6.5), Inches(2.6))
tbl = tbl_shape.table
data = [["工厂", "产能（t/d)", "状态"], ["苏州", "120", "在产"], ["常州", "80", "在产"], ["宜春", "60", "建设中"]]
for r in range(rows):
    for c in range(cols):
        tbl.cell(r, c).text = data[r][c]
from pptx.enum.shapes import MSO_SHAPE
sh = s.shapes.add_shape(MSO_SHAPE.ROUNDED_RECTANGLE, Inches(8.0), Inches(1.4), Inches(3.6), Inches(1.4))
sh.text_frame.text = "重点工厂：苏州"
sh.fill.solid()
sh.fill.fore_color.rgb = RGBColor(0x1F, 0x6F, 0xB2)
ar = s.shapes.add_shape(MSO_SHAPE.RIGHT_ARROW, Inches(8.6), Inches(3.4), Inches(2.4), Inches(0.9))
ar.fill.solid()
ar.fill.fore_color.rgb = RGBColor(0xE8, 0x8B, 0x2E)
notes(s, "表格备注：宜春厂明年投产。")

# 6 — picture + entrance animation (injected timing) + transition
s = prs.slides.add_slide(blank)
add_title(s, "厂区图（图片 + 进入动画 + 切换效果）")
# poster/photo: gradient PNG via PIL
from PIL import Image, ImageDraw
img = Image.new("RGB", (960, 540), (24, 64, 110))
d = ImageDraw.Draw(img)
for i in range(540):
    d.line([(0, i), (960, i)], fill=(24 + i // 8, 64 + i // 6, 110 + i // 5))
d.rectangle([120, 320, 840, 500], fill=(232, 139, 46))
d.text((140, 340), "PLANT PHOTO", fill=(255, 255, 255))
img.save("/tmp/case2-photo.png")
pic = s.shapes.add_picture("/tmp/case2-photo.png", Inches(1.2), Inches(1.3), Inches(7.5), Inches(4.2))
spid = pic._element.nvPicPr.cNvPr.get("id")
P = "http://schemas.openxmlformats.org/presentationml/2006/main"
timing_xml = f'''<p:timing xmlns:p="{P}"><p:tnLst><p:par><p:cTn id="1" dur="indefinite" restart="never" nodeType="tmRoot"><p:childTnLst><p:seq concurrent="1" nextAc="seek"><p:cTn id="2" dur="indefinite" nodeType="mainSeq"><p:childTnLst><p:par><p:cTn id="3" fill="hold"><p:stCondLst><p:cond delay="indefinite" evt="onClick"/></p:stCondLst><p:childTnLst><p:par><p:cTn id="4" presetID="1" presetClass="entr" presetSubtype="0" fill="hold" grpId="0" nodeType="clickEffect"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst><p:set><p:cBhvr><p:cTn id="5" dur="1" fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst></p:cTn><p:tgtEl><p:spTgt spid="{spid}"/></p:tgtEl><p:attrNameLst><p:attrName>style.visibility</p:attrName></p:attrNameLst></p:cBhvr><p:to><p:strVal val="visible"/></p:to></p:set></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn><p:prevCondLst><p:cond evt="onPrev" delay="0"><p:tgtEl><p:sldTgt/></p:tgtEl></p:cond></p:prevCondLst><p:nextCondLst><p:cond evt="onNext" delay="0"><p:tgtEl><p:sldTgt/></p:tgtEl></p:cond></p:nextCondLst></p:seq></p:childTnLst></p:cTn></p:par></p:tnLst></p:timing>'''
slide_el = s._element
timing_el = etree.fromstring(timing_xml)
trans_el = etree.fromstring(f'<p:transition xmlns:p="{P}" spd="med" dur="700"><p:fade thruBlk="1"/></p:transition>')
# schema order: cSld, transition, timing
slide_el.insert(1, trans_el)
slide_el.append(timing_el)
notes(s, "动画备注：图片单击后出现；本页带淡入切换。")

# 7 — video if ffmpeg exists
made_video = False
if subprocess.run(["which", "ffmpeg"], capture_output=True).returncode == 0:
    subprocess.run(["ffmpeg", "-y", "-f", "lavfi", "-i", "color=c=0x1F6FB2:s=640x360:d=2",
                    "-pix_fmt", "yuv420p", "/tmp/case2-clip.mp4"], capture_output=True)
    s = prs.slides.add_slide(blank)
    add_title(s, "工艺视频（嵌入媒体）")
    s.shapes.add_movie("/tmp/case2-clip.mp4", Inches(1.2), Inches(1.3), Inches(7.5), Inches(4.2),
                       poster_frame_image="/tmp/case2-photo.png", mime_type="video/mp4")
    notes(s, "视频备注：2 秒纯色测试片。")
    made_video = True
else:
    print("ffmpeg missing — video slide skipped", file=sys.stderr)

prs.save(OUT)
print("saved", OUT, "| slides:", len(prs.slides.__iter__.__self__._sldIdLst), "| video:", made_video)
