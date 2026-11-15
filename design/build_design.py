#!/usr/bin/env python3
"""Build 启幕 UI design boards in FigCraft via its control channel (port 7991).
Design spec: design/设计规范-EffectCraft风格.md (dark + blue, from EffectCraft)."""
import socket, json, sys

S = socket.create_connection(('127.0.0.1', 7991), timeout=60)
F = S.makefile('rw', encoding='utf-8', newline='\n')
_n = [0]

def call(method, params=None):
    _n[0] += 1
    F.write(json.dumps({"id": _n[0], "method": method, "params": params or {}}) + "\n")
    F.flush()
    line = F.readline()
    r = json.loads(line)
    if not r.get('ok', True):
        print('ERR', method, r.get('error'))
    return r

def ex(cmd, params=None):
    r = call("engine.execute", {"command": cmd, "params": params or {}})
    return r.get('result', {}).get('id')

# palette
BG = "#121212"; CHROME = "#1C1C1C"; PANEL = "#1C1C1C"; CARD = "#232323"
BORDER = "#383838"; FIELD = "#2A2A2A"; BTN2 = "#303030"; SEL = "#3A3A3A"
ACCENT = "#2D8CEB"; ACCENT_TILE = "#1B2A3F"
T1 = "#F2F2F2"; T2 = "#9E9E9E"; T3 = "#6E6E6E"
GREEN = "#35B26B"; GREEN_BG = "#1E3A2A"; ORANGE = "#E8A13A"; ORANGE_BG = "#3D2E1A"

def rect(parent, x, y, w, h, fill, r=0, stroke=None, name=None):
    props = {"fill": fill}
    if r: props["cornerRadius"] = r
    if stroke: props["stroke"] = stroke; props["strokeWeight"] = 1
    return ex("shape.rectangle", {"parent": parent, "x": x, "y": y, "width": w,
                                  "height": h, "props": props, "name": name or "rect"})

def text(parent, x, y, s, size=13, color=T1, weight=None, width=None, align=None):
    props = {"fontSize": size, "fill": color}
    if weight: props["fontWeight"] = weight
    if align: props["textAlign"] = align
    p = {"parent": parent, "x": x, "y": y, "characters": s, "props": props, "name": "t"}
    if width: p["width"] = width
    return ex("text.create", p)

def frame(x, name):
    fid = ex("frame.create", {"x": x, "y": 0, "width": 1440, "height": 900, "name": name})
    rect(fid, 0, 0, 1440, 900, BG)
    return fid

def topbar(fid, active):
    rect(fid, 0, 0, 1440, 52, CHROME)
    rect(fid, 16, 14, 24, 24, ACCENT, r=6)
    text(fid, 48, 13, "启幕", 19, T1, weight=600)
    tabs = ["演示厅", "内容页", "索引页", "设置"]
    x = 128
    for t in tabs:
        w = 17 * len(t) + 16
        c = T1 if t == active else T2
        text(fid, x + 8, 16, t, 14, c, weight=600 if t == active else None)
        if t == active:
            rect(fid, x + 8, 44, 17 * len(t), 2, ACCENT)
        x += w + 14
    text(fid, 1130, 17, "超级管理员 · super", 12, T2)
    rect(fid, 1270, 12, 56, 28, BTN2, r=8); text(fid, 1284, 17, "刷新", 12, T1)
    rect(fid, 1336, 12, 56, 28, BTN2, r=8); text(fid, 1350, 17, "退出", 12, T1)
    rect(fid, 0, 52, 1440, 1, "#2A2A2A")

def chip(fid, x, y, s, fg, bg):
    w = 10 * len(s) + 15
    rect(fid, x, y, w, 18, bg, r=9)
    text(fid, x + 8, y + 2, s, 11, fg)
    return w

def slide_art(fid, x, y, w, kind):
    """Mini slide mock (bright slide on dark UI): w wide, 16:9."""
    h = w * 9 / 16
    rect(fid, x, y, w, h, "#FFFFFF")
    if kind == "cover":
        rect(fid, x + 14, y + 12, w * 0.42, 7, "#1F3864")
        rect(fid, x + 14, y + 30, w * 0.30, 5, "#8EAADB")
        rect(fid, x + 14, y + 44, w * 0.22, 5, "#C9D4E8")
        rect(fid, x + w * 0.58, y + 26, w * 0.34, h * 0.52, "#B8C4D4")
        rect(fid, x + w - 26, y + 10, 12, 12, "#E36C0A")
    elif kind == "chart":
        rect(fid, x + 14, y + 12, w * 0.5, 7, "#1F3864")
        base = y + h - 22
        for i, bh in enumerate([0.35, 0.55, 0.42, 0.68, 0.5]):
            rect(fid, x + 30 + i * (w * 0.16), base - h * bh * 0.55, w * 0.09, h * bh * 0.55,
                 ["#2D8CEB", "#E36C0A", "#35B26B", "#7A5AF8", "#E8A13A"][i])
        rect(fid, x + 20, base, w - 40, 1, "#CCCCCC")
    elif kind == "smart":
        rect(fid, x + 14, y + 12, w * 0.44, 7, "#1F3864")
        for i in range(3):
            rect(fid, x + 24 + i * (w * 0.3), y + h * 0.38, w * 0.24, h * 0.3, "#2E5AAC", r=6)
    else:  # table/text
        rect(fid, x + 14, y + h * 0.08, w * 0.52, h * 0.07, "#1F3864")
        rect(fid, x + 14, y + h * 0.22, w - 28, h * 0.11, "#1F3864")
        for i in range(3):
            rect(fid, x + 14, y + h * (0.40 + i * 0.15), w - 28, h * 0.08, "#EDEFF3")
    return h

def content_card(fid, x, y, title, ver, pages, meta, kind):
    rect(fid, x, y, 300, 258, CARD, r=10, stroke=BORDER)
    slide_art(fid, x, y, 300, kind)
    text(fid, x + 14, y + 184, title, 15, T1, weight=600)
    chip(fid, x + 14, y + 216, "v" + ver, ACCENT, ACCENT_TILE)
    text(fid, x + 14 + 11 * (len(ver) + 1) + 24, y + 217, f"{pages} 页", 11, T2)
    text(fid, x + 300 - 14 - 11 * len(meta), y + 217, meta, 11, T3)

def screen_content(ox):
    fid = frame(ox, "内容页")
    topbar(fid, "内容页")
    rect(fid, 0, 53, 208, 847, PANEL)
    rows = [("全部", True), ("公司介绍", False), ("公司技术", False), ("公司案例", False)]
    for i, (name, sel) in enumerate(rows):
        if sel: rect(fid, 8, 65 + i * 40, 192, 34, SEL, r=8)
        text(fid, 20, 72 + i * 40, name, 13, T1 if sel else T2)
    text(fid, 20, 240, "本地缓存", 12, T3)
    # toolbar
    rect(fid, 232, 65, 300, 32, FIELD, r=16)
    text(fid, 248, 73, "搜索", 12, T3)
    text(fid, 556, 71, "智能分块导入", 13, T1)
    text(fid, 660, 71, "导入 PPT", 13, T1)
    text(fid, 736, 71, "导入到组", 13, T1)
    rect(fid, 812, 65, 150, 32, FIELD, r=8)
    text(fid, 824, 73, "公司介绍", 12, T2)
    rect(fid, 232, 109, 300, 32, FIELD, r=8)
    text(fid, 248, 117, "新建内容块", 12, T3)
    rect(fid, 544, 109, 64, 32, BTN2, r=8); text(fid, 560, 116, "创建", 12, T1)
    cards = [
        ("封面模板", "1.0.0", 6, "· 超级管理员", "cover"),
        ("开场部分", "1.0.0", 6, "公司介绍 · 超级管理员", "cover"),
        ("技术部分", "1.1.0", 5, "公司介绍 · 超级管理员", "table"),
        ("收尾部分", "1.0.0", 5, "公司介绍 · 超级管理员", "table"),
        ("案例2图表", "1.4.0", 7, "公司介绍 · 超级管理员", "chart"),
        ("案例2SmartArt", "1.0.0", 1, "公司介绍 · 超级管理员", "smart"),
    ]
    for i, (t, v, p, m, k) in enumerate(cards):
        cx = 232 + (i % 3) * 316
        cy = 157 + (i // 3) * 274
        content_card(fid, cx, cy, t, v, p, m, k)
    return fid

if __name__ == "__main__":
    fid = screen_content(1600)
    r = call("ui.render", {"path": "/home/hatch/workspace/qimu/design/内容页.png", "scale": 1, "ids": [fid]})
    print(r.get('result'))

def screen_showroom(ox):
    fid = frame(ox, "演示厅")
    topbar(fid, "演示厅")
    zones = ["广场", "我的库", "收到的分享", "群组空间"]
    x = 24
    for z in zones:
        c = T1 if z == "广场" else T2
        text(fid, x, 68, z, 14, c, weight=600 if z == "广场" else None)
        if z == "广场":
            rect(fid, x, 92, 17 * len(z), 2, ACCENT)
        x += 17 * len(z) + 28
    cards = [
        ("Redwood 来访介绍", "电池回收技术交流 · 17 页", "超级管理员 · 2026-11-15", "cover"),
        ("公司介绍 2026", "公司整体介绍 · 24 页", "超级管理员 · 2026-11-12", "table"),
        ("案例2图表演示", "图表与动画保真 · 7 页", "技术部 · 2026-11-15", "chart"),
        ("产品方案 SmartArt", "流程图演示 · 1 页", "技术部 · 2026-11-14", "smart"),
    ]
    for i, (t, sub, meta, k) in enumerate(cards):
        cx = 24 + i * 356
        rect(fid, cx, 116, 340, 300, CARD, r=10, stroke=BORDER)
        slide_art(fid, cx, 116, 340, k)
        text(fid, cx + 16, 116 + 205, t, 15, T1, weight=600)
        text(fid, cx + 16, 116 + 230, sub, 12, T2)
        text(fid, cx + 16, 116 + 250, meta, 11, T3)
        text(fid, cx + 16, 116 + 272, "开始演示", 12, ACCENT)
        text(fid, cx + 84, 116 + 272, "分享", 12, T1)
        text(fid, cx + 128, 116 + 272, "缓存到本地", 12, T1)
    return fid

def entry_row(fid, y, num, name, ver, policy, locked):
    rect(fid, 232, y, 1184, 118, CARD, r=10, stroke=BORDER)
    text(fid, 248, y + 12, f"#{num}", 12, T2)
    for i, k in enumerate(["cover", "table", "smart"]):
        slide_art(fid, 248 + i * 138, y + 30, 128, k)
    text(fid, 248, y + 96, "共 5 页" if num != 1 else "共 6 页", 11, T3)
    text(fid, 690, y + 16, name, 15, T1, weight=600)
    text(fid, 690, y + 40, f"标准 · en · v{ver}", 12, T2)
    # quick version strip
    for i, v in enumerate(["1.1.0", "1.0.0"]):
        sx = 690 + i * 74
        rect(fid, sx, y + 66, 66, 38, "#FFFFFF", r=4)
        rect(fid, sx + 5, y + 71, 30, 4, "#1F3864")
        rect(fid, sx + 5, y + 79, 44, 3, "#C9D4E8")
        text(fid, sx, y + 106, "", 10, T3)
    if locked:
        chip(fid, 1050, y + 18, "版本锁定", ORANGE, ORANGE_BG)
        text(fid, 1230, y + 18, "解除锁定", 12, GREEN)
    else:
        chip(fid, 1050, y + 18, "跟随最新", GREEN, GREEN_BG)
        text(fid, 1230, y + 18, "锁定当前版本", 12, T1)
    text(fid, 1330, y + 18, "删除", 12, T1)
    text(fid, 1372, y + 18, "选页", 12, T1)
    text(fid, 1050, y + 48, "更换版本", 12, T1)
    text(fid, 1130, y + 48, "保持最新", 12, GREEN if not locked else T2)
    text(fid, 1210, y + 48, "去内容页维护", 12, T1)

def screen_index(ox):
    fid = frame(ox, "索引页")
    topbar(fid, "索引页")
    rect(fid, 0, 53, 208, 847, PANEL)
    text(fid, 20, 68, "+ 新建索引", 13, ACCENT)
    items = ["Redwood 来访介绍", "案例2图表 索引", "模板导出测试", "公司介绍（主索引）"]
    for i, name in enumerate(items):
        if i == 0: rect(fid, 8, 96 + i * 40, 192, 34, SEL, r=8)
        text(fid, 20, 103 + i * 40, name, 13, T1 if i == 0 else T2)
    text(fid, 232, 68, "Redwood 来访介绍", 19, T1, weight=600)
    text(fid, 460, 73, "· 超级管理员", 12, T2)
    chip(fid, 232, 100, "公司介绍 › 客户拜访", T2, BTN2)
    text(fid, 420, 101, "分支", 12, T2)
    rect(fid, 452, 96, 90, 26, FIELD, r=6); text(fid, 464, 102, "main", 12, T1)
    text(fid, 560, 101, "版本 V1.0.0", 12, T2)
    rect(fid, 660, 94, 200, 30, FIELD, r=8)
    rect(fid, 872, 94, 84, 30, ACCENT, r=15); text(fid, 886, 101, "发布版本", 12, "#FFFFFF")
    text(fid, 976, 101, "导出（个人版）", 12, T1)
    text(fid, 1090, 101, "推送到演示厅", 12, T1)
    entry_row(fid, 140, 1, "开场部分", "1.0.0", "follow", False)
    entry_row(fid, 270, 2, "技术部分", "1.1.0", "lock", True)
    entry_row(fid, 400, 3, "收尾部分", "1.0.0", "follow", False)
    text(fid, 232, 545, "提交历史", 14, T1, weight=600)
    for i, line in enumerate([
        "合并分支 feature（源提交 cmt_13221569）· 超级管理员 · 2026-11-15 10:22",
        "技术部分只用第一页 · 超级管理员 · 2026-11-15 10:22",
        "发布 V1.0.0 · 超级管理员 · 2026-11-15 08:47",
    ]):
        text(fid, 232, 575 + i * 24, line, 12, T2)
    return fid

def screen_login(ox):
    fid = frame(ox, "登录")
    rect(fid, 470, 225, 500, 400, CARD, r=16, stroke=BORDER)
    rect(fid, 688, 262, 64, 64, ACCENT, r=14)
    text(fid, 706, 278, "启", 30, "#FFFFFF", weight=600)
    text(fid, 672, 340, "启幕", 26, T1, weight=600)
    text(fid, 649, 378, "PPT 内容管理与更新系统", 13, T2)
    labels = [("服务器", "http://192.168.1.10:8730"), ("账号", "admin"), ("密码", "••••••••")]
    for i, (lb, val) in enumerate(labels):
        y = 420 + i * 52
        text(fid, 510, y + 7, lb, 13, T2)
        rect(fid, 570, y, 330, 34, FIELD, r=8)
        text(fid, 582, y + 8, val, 13, T1)
    rect(fid, 570, 420 + 3 * 52, 330, 40, ACCENT, r=20)
    text(fid, 706, 420 + 3 * 52 + 10, "登录", 14, "#FFFFFF", weight=600)
    return fid

def screen_settings(ox):
    fid = frame(ox, "设置")
    topbar(fid, "设置")
    rect(fid, 0, 53, 208, 847, PANEL)
    items = ["个人资料", "组织管理", "内容组与授权", "公司公共资料", "内容语言", "AI 翻译", "离线缓存策略"]
    for i, name in enumerate(items):
        if i == 0: rect(fid, 8, 65 + i * 40, 192, 34, SEL, r=8)
        text(fid, 20, 72 + i * 40, name, 13, T1 if i == 0 else T2)
    text(fid, 232, 68, "个人资料", 19, T1, weight=600)
    fields = [("姓名", "超级管理员"), ("职务", "技术总监"), ("英文名", "Admin"),
              ("手机号", "138 0000 1111"), ("邮箱", "admin@example.com"), ("微信二维码", "已上传")]
    for i, (lb, val) in enumerate(fields):
        y = 116 + i * 54
        text(fid, 232, y + 8, lb, 13, T2)
        rect(fid, 340, y, 380, 34, FIELD, r=8)
        text(fid, 352, y + 9, val, 13, T1)
    rect(fid, 340, 116 + 6 * 54, 120, 38, ACCENT, r=19)
    text(fid, 376, 116 + 6 * 54 + 10, "保存资料", 13, "#FFFFFF", weight=600)
    text(fid, 800, 68, "个人绑定预览", 14, T1, weight=600)
    rect(fid, 800, 100, 340, 191, "#FFFFFF", r=8)
    rect(fid, 814, 112, 150, 8, "#1F3864")
    text(fid, 814, 132, "汇报人：超级管理员 · 技术总监", 11, "#333333")
    text(fid, 814, 152, "138 0000 1111 · admin@example.com", 11, "#666666")
    rect(fid, 814, 190, 90, 60, "#B8C4D4")
    return fid

def entry_row_v2(fid, y, num, name, ver, pages, locked, strip):
    rect(fid, 232, y, 1184, 150, CARD, r=10, stroke=BORDER)
    text(fid, 1400, y + 10, "删除", 11, T2)  # placeholder replaced below
    text(fid, 248, y + 10, f"#{num}", 12, T2)
    kinds = ["cover", "table", "smart", "chart", "table", "cover"]
    for i in range(pages):
        slide_art(fid, 248 + i * 92, y + 30, 86, kinds[i % len(kinds)])
    text(fid, 248, y + 88, f"共 {pages} 页", 11, T3)
    text(fid, 812, y + 12, name, 15, T1, weight=600)
    text(fid, 812, y + 36, f"标准 · en · v{ver} · 超级管理员", 12, T2)
    if locked:
        chip(fid, 812, y + 58, "版本锁定", ORANGE, ORANGE_BG)
        ax = 812 + 10 * 4 + 26
        text(fid, ax, y + 59, "更换版本", 12, T1)
        text(fid, ax + 62, y + 58, "解除锁定", 12, GREEN)
        text(fid, ax + 124, y + 59, "去内容页维护", 12, T1)
    else:
        chip(fid, 812, y + 58, "跟随最新", GREEN, GREEN_BG)
        ax = 812 + 10 * 4 + 26
        text(fid, ax, y + 59, "更换版本", 12, T1)
        rect(fid, ax + 62, y + 55, 66, 24, GREEN_BG, r=6)
        text(fid, ax + 71, y + 60, "保持最新", 12, GREEN)
        text(fid, ax + 140, y + 59, "去内容页维护", 12, T1)
    # quick-switch strip: history version minis, newest -> oldest, + more
    sx = 812
    for v in strip:
        rect(fid, sx, y + 92, 54, 30, "#FFFFFF", r=3)
        rect(fid, sx + 4, y + 96, 22, 3, "#1F3864")
        rect(fid, sx + 4, y + 102, 34, 2, "#C9D4E8")
        text(fid, sx + 2, y + 126, "v" + v, 10, T2)
        sx += 66
    text(fid, sx + 2, y + 100, "更多 · +2", 11, ACCENT)
    # delete at top-right corner
    rect(fid, 1368, y + 8, 40, 20, CARD)
    text(fid, 1372, y + 11, "删除", 11, T2)

def screen_index_v2(ox):
    fid = frame(ox, "索引页")
    topbar(fid, "索引页")
    rect(fid, 0, 53, 208, 847, PANEL)
    text(fid, 20, 68, "+ 新建索引", 13, ACCENT)
    items = ["Redwood 来访介绍", "案例2图表 索引", "模板导出测试", "公司介绍（主索引）"]
    for i, name in enumerate(items):
        if i == 0: rect(fid, 8, 96 + i * 40, 192, 34, SEL, r=8)
        text(fid, 20, 103 + i * 40, name, 13, T1 if i == 0 else T2)
    # header row 1: back + five views
    text(fid, 232, 66, "← 返回", 13, T2)
    views = ["条目图墙", "提交图", "历史版本", "导出记录", "派生关系"]
    x = 320
    for v in views:
        c = T1 if v == "条目图墙" else T2
        text(fid, x, 66, v, 13, c, weight=600 if v == "条目图墙" else None)
        if v == "条目图墙":
            rect(fid, x, 88, 17 * len(v), 2, ACCENT)
        x += 17 * len(v) + 24
    # header row 2: title + coordinate
    text(fid, 232, 100, "Redwood 来访介绍", 19, T1, weight=600)
    chip(fid, 452, 104, "主索引", ACCENT, ACCENT_TILE)
    text(fid, 530, 105, "公司介绍 › 客户拜访", 12, T2)
    # header row 3: branch/tag + actions
    text(fid, 232, 140, "分支", 12, T2)
    rect(fid, 264, 134, 90, 26, FIELD, r=6); text(fid, 276, 140, "main", 12, T1)
    text(fid, 370, 140, "版本 V1.0.0", 12, T2)
    text(fid, 940, 139, "拉分支", 12, T1)
    text(fid, 1004, 139, "发起合并", 12, T1)
    text(fid, 1088, 139, "更多", 12, T1)
    text(fid, 1140, 139, "推送到演示厅", 12, T1)
    rect(fid, 1256, 132, 160, 30, ACCENT, r=15)
    text(fid, 1296, 139, "发布新版本", 12, "#FFFFFF", weight=600)
    entry_row_v2(fid, 176, 1, "开场部分", "1.0.0", 6, False, ["1.0.0"])
    entry_row_v2(fid, 340, 2, "技术部分", "1.1.0", 5, True, ["1.1.0", "1.0.0"])
    entry_row_v2(fid, 504, 3, "收尾部分", "1.0.0", 5, False, ["1.0.0"])
    text(fid, 232, 680, "提交历史", 14, T1, weight=600)
    for i, line in enumerate([
        "合并分支 feature（源提交 cmt_13221569）· 超级管理员 · 2026-11-15 10:22",
        "技术部分只用第一页 · 超级管理员 · 2026-11-15 10:22",
        "发布 V1.0.0 · 超级管理员 · 2026-11-15 08:47",
    ]):
        text(fid, 232, 708 + i * 24, line, 12, T2)
    return fid

def entry_row_v3(fid, y, num, name, ver, pages, locked, strip, more_n=0):
    rect(fid, 232, y, 1184, 142, CARD, r=10, stroke=BORDER)
    text(fid, 248, y + 10, f"#{num}", 12, T2)
    kinds = ["cover", "table", "smart", "chart", "table", "cover"]
    for i in range(pages):
        slide_art(fid, 248 + i * 92, y + 28, 86, kinds[i % len(kinds)])
    text(fid, 248, y + 82, f"共 {pages} 页", 11, T3)
    text(fid, 812, y + 12, name, 15, T1, weight=600)
    text(fid, 812, y + 35, f"标准 · en · v{ver} · 超级管理员", 12, T2)
    # ONE action row: policy chip + all actions on a single line
    ax = 812
    if locked:
        w = chip(fid, ax, y + 58, "版本锁定", ORANGE, ORANGE_BG)
    else:
        w = chip(fid, ax, y + 58, "跟随最新", GREEN, GREEN_BG)
    ax += w + 14
    text(fid, ax, y + 59, "更换版本", 12, T1); ax += 62
    if locked:
        text(fid, ax, y + 59, "解除锁定", 12, GREEN); ax += 62
    else:
        rect(fid, ax - 4, y + 55, 66, 24, GREEN_BG, r=6)
        text(fid, ax + 5, y + 60, "保持最新", 12, GREEN); ax += 76
    text(fid, ax, y + 59, "去内容页维护", 12, T1); ax += 86
    text(fid, ax, y + 59, "删除", 12, T1); ax += 38
    text(fid, ax, y + 59, "选页", 12, T1)
    # quick-switch strip
    sx = 812
    for v in strip:
        rect(fid, sx, y + 90, 54, 30, "#FFFFFF", r=3)
        rect(fid, sx + 4, y + 94, 22, 3, "#1F3864")
        rect(fid, sx + 4, y + 100, 34, 2, "#C9D4E8")
        text(fid, sx + 2, y + 123, "v" + v, 10, T2)
        sx += 66
    if more_n:
        text(fid, sx + 2, y + 98, f"更多 · +{more_n}", 11, ACCENT)

def screen_index_v3(ox):
    fid = frame(ox, "索引页")
    topbar(fid, "索引页")
    rect(fid, 0, 53, 208, 847, PANEL)
    text(fid, 20, 68, "+ 新建索引", 13, ACCENT)
    items = ["Redwood 来访介绍", "案例2图表 索引", "模板导出测试", "公司介绍（主索引）"]
    for i, name in enumerate(items):
        if i == 0: rect(fid, 8, 96 + i * 40, 192, 34, SEL, r=8)
        text(fid, 20, 103 + i * 40, name, 13, T1 if i == 0 else T2)
    text(fid, 232, 66, "← 返回", 13, T2)
    views = ["条目图墙", "提交图", "历史版本", "导出记录", "派生关系"]
    x = 320
    for v in views:
        c = T1 if v == "条目图墙" else T2
        text(fid, x, 66, v, 13, c, weight=600 if v == "条目图墙" else None)
        if v == "条目图墙":
            rect(fid, x, 88, 17 * len(v), 2, ACCENT)
        x += 17 * len(v) + 24
    text(fid, 232, 100, "Redwood 来访介绍", 19, T1, weight=600)
    chip(fid, 452, 104, "主索引", ACCENT, ACCENT_TILE)
    text(fid, 530, 105, "公司介绍 › 客户拜访", 12, T2)
    text(fid, 232, 140, "分支", 12, T2)
    rect(fid, 264, 134, 90, 26, FIELD, r=6); text(fid, 276, 140, "main", 12, T1)
    text(fid, 370, 140, "版本 V1.0.0", 12, T2)
    text(fid, 940, 139, "拉分支", 12, T1)
    text(fid, 1004, 139, "发起合并", 12, T1)
    text(fid, 1088, 139, "更多", 12, T1)
    text(fid, 1140, 139, "推送到演示厅", 12, T1)
    rect(fid, 1256, 132, 160, 30, ACCENT, r=15)
    text(fid, 1296, 139, "发布新版本", 12, "#FFFFFF", weight=600)
    entry_row_v3(fid, 176, 1, "开场部分", "1.0.0", 6, False, ["1.0.0"])
    entry_row_v3(fid, 332, 2, "技术部分", "1.1.0", 5, True, ["1.1.0", "1.0.0"], more_n=1)
    entry_row_v3(fid, 488, 3, "收尾部分", "1.0.0", 5, False, ["1.0.0"])
    text(fid, 232, 656, "提交历史", 14, T1, weight=600)
    for i, line in enumerate([
        "合并分支 feature（源提交 cmt_13221569）· 超级管理员 · 2026-11-15 10:22",
        "技术部分只用第一页 · 超级管理员 · 2026-11-15 10:22",
        "发布 V1.0.0 · 超级管理员 · 2026-11-15 08:47",
    ]):
        text(fid, 232, 684 + i * 24, line, 12, T2)
    return fid

def entry_row_v4(fid, y, num, name, ver, pages, locked, strip, more_n=0):
    rect(fid, 232, y, 1184, 192, CARD, r=10, stroke=BORDER)
    text(fid, 248, y + 10, f"#{num}", 12, T2)
    kinds = ["cover", "table", "smart", "chart", "table", "cover", "smart"]
    # left: first page LARGE by default; extra pages as small thumbs beside it
    slide_art(fid, 248, y + 30, 232, kinds[0])
    for i in range(1, pages):
        col, row = (i - 1) % 3, (i - 1) // 3
        slide_art(fid, 492 + col * 92, y + 30 + row * 54, 86, kinds[i % len(kinds)])
    text(fid, 248, y + 166, f"共 {pages} 页", 11, T3)
    # right: info + one-line actions + quick strip
    text(fid, 790, y + 14, name, 15, T1, weight=600)
    text(fid, 790, y + 38, f"标准 · en · v{ver} · 超级管理员", 12, T2)
    ax = 790
    if locked:
        w = chip(fid, ax, y + 62, "版本锁定", ORANGE, ORANGE_BG)
    else:
        w = chip(fid, ax, y + 62, "跟随最新", GREEN, GREEN_BG)
    ax += w + 14
    text(fid, ax, y + 63, "更换版本", 12, T1); ax += 62
    if locked:
        text(fid, ax, y + 63, "解除锁定", 12, GREEN); ax += 62
    else:
        rect(fid, ax - 4, y + 59, 66, 24, GREEN_BG, r=6)
        text(fid, ax + 5, y + 64, "保持最新", 12, GREEN); ax += 76
    text(fid, ax, y + 63, "去内容页维护", 12, T1); ax += 86
    text(fid, ax, y + 63, "删除", 12, T1); ax += 38
    text(fid, ax, y + 63, "选页", 12, T1)
    sx = 790
    for v in strip:
        rect(fid, sx, y + 98, 54, 30, "#FFFFFF", r=3)
        rect(fid, sx + 4, y + 102, 22, 3, "#1F3864")
        rect(fid, sx + 4, y + 108, 34, 2, "#C9D4E8")
        text(fid, sx + 2, y + 131, "v" + v, 10, T2)
        sx += 66
    if more_n:
        text(fid, sx + 2, y + 106, f"更多 · +{more_n}", 11, ACCENT)

def screen_index_v4(ox):
    fid = frame(ox, "索引页")
    topbar(fid, "索引页")
    rect(fid, 0, 53, 208, 847, PANEL)
    text(fid, 20, 68, "+ 新建索引", 13, ACCENT)
    items = ["Redwood 来访介绍", "案例2图表 索引", "模板导出测试", "公司介绍（主索引）"]
    for i, name in enumerate(items):
        if i == 0: rect(fid, 8, 96 + i * 40, 192, 34, SEL, r=8)
        text(fid, 20, 103 + i * 40, name, 13, T1 if i == 0 else T2)
    text(fid, 232, 66, "← 返回", 13, T2)
    views = ["条目图墙", "提交图", "历史版本", "导出记录", "派生关系"]
    x = 320
    for v in views:
        c = T1 if v == "条目图墙" else T2
        text(fid, x, 66, v, 13, c, weight=600 if v == "条目图墙" else None)
        if v == "条目图墙":
            rect(fid, x, 88, 17 * len(v), 2, ACCENT)
        x += 17 * len(v) + 24
    text(fid, 232, 100, "Redwood 来访介绍", 19, T1, weight=600)
    chip(fid, 452, 104, "主索引", ACCENT, ACCENT_TILE)
    text(fid, 530, 105, "公司介绍 › 客户拜访", 12, T2)
    text(fid, 232, 140, "分支", 12, T2)
    rect(fid, 264, 134, 90, 26, FIELD, r=6); text(fid, 276, 140, "main", 12, T1)
    text(fid, 370, 140, "版本 V1.0.0", 12, T2)
    text(fid, 940, 139, "拉分支", 12, T1)
    text(fid, 1004, 139, "发起合并", 12, T1)
    text(fid, 1088, 139, "更多", 12, T1)
    text(fid, 1140, 139, "推送到演示厅", 12, T1)
    rect(fid, 1256, 132, 160, 30, ACCENT, r=15)
    text(fid, 1296, 139, "发布新版本", 12, "#FFFFFF", weight=600)
    entry_row_v4(fid, 168, 1, "开场部分", "1.0.0", 6, False, ["1.0.0"])
    entry_row_v4(fid, 376, 2, "技术部分", "1.1.0", 5, True, ["1.1.0", "1.0.0"], more_n=1)
    entry_row_v4(fid, 584, 3, "收尾部分", "1.0.0", 1, False, ["1.0.0"])
    text(fid, 232, 792, "提交历史", 14, T1, weight=600)
    for i, line in enumerate([
        "合并分支 feature（源提交 cmt_13221569）· 超级管理员 · 2026-11-15 10:22",
        "技术部分只用第一页 · 超级管理员 · 2026-11-15 10:22",
        "发布 V1.0.0 · 超级管理员 · 2026-11-15 08:47",
    ]):
        text(fid, 232, 818 + i * 24, line, 12, T2)
    return fid
