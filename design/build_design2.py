#!/usr/bin/env python3
"""启幕 design boards v2 — current layouts (folder tree left, content right).
Built in FigCraft via control channel :7991. Boards: 登录 / 演示厅 / 内容页 /
索引总览 / 索引详情 / 设置. Saves qimu-ui.figcraft + renders PNGs."""
import socket, json, os

S = socket.create_connection(('127.0.0.1', 7991), timeout=120)
F = S.makefile('rw', encoding='utf-8', newline='\n')
_n = [0]

def call(method, params=None):
    _n[0] += 1
    F.write(json.dumps({"id": _n[0], "method": method, "params": params or {}}) + "\n")
    F.flush()
    r = json.loads(F.readline())
    if not r.get('ok', True):
        print('ERR', method, r.get('error'))
    return r

def ex(cmd, params=None):
    r = call("engine.execute", {"command": cmd, "params": params or {}})
    return r.get('result', {}).get('id')

BG = "#121212"; CHROME = "#232323"; PANEL = "#232323"; CARD = "#282828"
BORDER = "#383838"; FIELD = "#2A2A2A"; BTN2 = "#303030"; SEL = "#3A3A3A"
ACCENT = "#2D8CEB"; ACCENT_TILE = "#1B2A3F"; RED = "#E5534B"
T1 = "#F2F2F2"; T2 = "#9E9E9E"; T3 = "#6E6E6E"
GREEN = "#35B26B"; GREEN_BG = "#1E3A2A"; ORANGE = "#E8A13A"; ORANGE_BG = "#3D2E1A"

def rect(parent, x, y, w, h, fill, r=0, stroke=None, name=None):
    props = {"fill": fill}
    if r: props["cornerRadius"] = r
    if stroke: props["stroke"] = stroke; props["strokeWeight"] = 1
    return ex("shape.rectangle", {"parent": parent, "x": x, "y": y, "width": w,
                                  "height": h, "props": props, "name": name or "rect"})

def text(parent, x, y, s, size=13, color=T1, weight=None):
    props = {"fontSize": size, "fill": color}
    if weight: props["fontWeight"] = weight
    return ex("text.create", {"parent": parent, "x": x, "y": y, "characters": s,
                              "props": props, "name": "t"})

def frame(x, name):
    fid = ex("frame.create", {"x": x, "y": 0, "width": 1440, "height": 900, "name": name})
    rect(fid, 0, 0, 1440, 900, BG)
    return fid

def topbar(fid, active):
    rect(fid, 0, 0, 1440, 52, "#1F1F1F")
    rect(fid, 16, 14, 24, 24, ACCENT, r=6)
    text(fid, 48, 13, "启幕", 19, T1, weight=600)
    x = 128
    for t in ["演示厅", "内容页", "索引页", "设置"]:
        c = T1 if t == active else T2
        text(fid, x + 8, 16, t, 14, c, weight=600 if t == active else None)
        if t == active:
            rect(fid, x + 8, 44, 17 * len(t), 2, ACCENT)
        x += 17 * len(t) + 30
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
    else:
        rect(fid, x + 14, y + h * 0.08, w * 0.52, h * 0.07, "#1F3864")
        rect(fid, x + 14, y + h * 0.22, w - 28, h * 0.11, "#1F3864")
        for i in range(3):
            rect(fid, x + 14, y + h * (0.40 + i * 0.15), w - 28, h * 0.08, "#EDEFF3")
    return h

def folder_panel(fid, selected, primary=None):
    """Left resident folder tree (220 wide). The primary action pill is
    pinned to the panel BOTTOM (James 2026-11-15); the tree sits above."""
    rect(fid, 0, 53, 220, 847, PANEL)
    y = 65
    rows = [("全部", None), ("公司介绍", 0), ("公司技术", 0), ("公司案例", 0)]
    for name, _ in rows:
        sel = (name == selected)
        if sel: rect(fid, 8, y, 204, 32, SEL, r=8)
        text(fid, 20, y + 7, name, 13.5, T1 if sel else T2)
        y += 38
    if primary:
        rect(fid, 10, 856, 200, 34, ACCENT, r=17)
        text(fid, 62, 864, primary, 13, "#FFFFFF", weight=600)

def tree_rows(fid, y, selected):
    rows = [("全部", None), ("公司介绍", 0), ("公司技术", 0), ("公司案例", 0)]
    for name, _ in rows:
        sel = (name == selected)
        if sel: rect(fid, 8, y, 204, 32, SEL, r=8)
        text(fid, 20, y + 7, name, 13.5, T1 if sel else T2)
        y += 38
    return y

# ---------------- 登录 ----------------
def screen_login(ox):
    fid = frame(ox, "登录")
    rect(fid, 470, 225, 500, 400, CARD, r=16, stroke=BORDER)
    rect(fid, 688, 262, 64, 64, ACCENT, r=14)
    text(fid, 706, 278, "启", 30, "#FFFFFF", weight=600)
    text(fid, 672, 340, "启幕", 26, T1, weight=600)
    text(fid, 649, 378, "PPT 内容管理与更新系统", 13, T2)
    for i, (lb, val) in enumerate([("服务器", "http://192.168.1.10:8730"),
                                   ("账号", "admin"), ("密码", "••••••••")]):
        y = 420 + i * 52
        text(fid, 510, y + 7, lb, 13, T2)
        rect(fid, 570, y, 330, 34, FIELD, r=8)
        text(fid, 582, y + 8, val, 13, T1)
    rect(fid, 570, 420 + 3 * 52, 330, 40, ACCENT, r=20)
    text(fid, 706, 420 + 3 * 52 + 10, "登录", 14, "#FFFFFF", weight=600)
    return fid

# ---------------- 演示厅 ----------------
def showroom_panel(fid, zone="广场", folder="全部"):
    """Showroom left panel (F): the four zones as vertical rows on top,
    the folder tree below them, separated by a hairline."""
    rect(fid, 0, 53, 220, 847, PANEL)
    y = 65
    for z in ["广场", "我的库", "收到的分享", "群组空间"]:
        if z == zone: rect(fid, 8, y, 204, 32, SEL, r=8)
        text(fid, 20, y + 7, z, 13.5, T1 if z == zone else T2)
        y += 38
    rect(fid, 16, y + 2, 188, 1, "#2A2A2A")
    tree_rows(fid, y + 12, folder)

def screen_showroom(ox):
    fid = frame(ox, "演示厅")
    topbar(fid, "演示厅")
    showroom_panel(fid)
    text(fid, 244, 66, "演示厅", 19, T1, weight=600)
    cards = [
        ("Redwood 来访介绍", "电池回收技术交流 · 17 页", "超级管理员 · 2026-11-15", "cover"),
        ("公司介绍 2026", "公司整体介绍 · 24 页", "超级管理员 · 2026-11-12", "table"),
        ("案例2图表演示", "图表与动画保真 · 7 页", "技术部 · 2026-11-15", "chart"),
    ]
    for i, (t, sub, meta, k) in enumerate(cards):
        cx = 244 + i * 356
        rect(fid, cx, 116, 340, 300, CARD, r=10, stroke=BORDER)
        slide_art(fid, cx, 116, 340, k)
        text(fid, cx + 16, 321, t, 15, T1, weight=600)
        text(fid, cx + 16, 346, sub, 12, T2)
        text(fid, cx + 16, 366, meta, 11, T3)
        text(fid, cx + 16, 388, "开始演示", 12, ACCENT)
        text(fid, cx + 84, 388, "分享", 12, T1)
        text(fid, cx + 128, 388, "缓存到本地", 12, T1)
    return fid

# ---------------- 内容页 ----------------
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
    folder_panel(fid, "全部")
    rect(fid, 244, 65, 300, 32, FIELD, r=16)
    text(fid, 260, 73, "搜索", 12, T3)
    text(fid, 568, 71, "智能分块导入", 13, T1)
    text(fid, 672, 71, "导入 PPT", 13, T1)
    text(fid, 748, 71, "导入到组", 13, T1)
    rect(fid, 824, 65, 150, 32, FIELD, r=8)
    text(fid, 836, 73, "公司介绍", 12, T2)
    cards = [
        ("开场部分", "1.0.0", 6, "公司介绍 · 超级管理员", "cover"),
        ("技术部分", "1.1.0", 5, "公司介绍 · 超级管理员", "table"),
        ("收尾部分", "1.0.0", 5, "公司介绍 · 超级管理员", "table"),
        ("案例2图表", "1.4.0", 7, "公司介绍 · 超级管理员", "chart"),
        ("案例2SmartArt", "1.0.0", 1, "公司介绍 · 超级管理员", "smart"),
        ("封面模板", "1.0.0", 6, "· 超级管理员", "cover"),
    ]
    for i, (t, v, p, m, k) in enumerate(cards):
        content_card(fid, 244 + (i % 3) * 316, 113 + (i // 3) * 274, t, v, p, m, k)
    # I: the new-block entry is a same-size add card at the grid's end
    ax, ay = 244, 113 + 2 * 274
    rect(fid, ax, ay, 300, 258, BG, r=10, stroke=BORDER)
    text(fid, ax + 138, ay + 96, "+", 36, T3)
    text(fid, ax + 112, ay + 148, "新建内容块", 13, T2)
    return fid

# ---------------- 索引总览 ----------------
def overview_card(fid, x, y, title, tag, meta, kind, border, hist=False, main=False):
    rect(fid, x, y, 300, 284, CARD, r=10, stroke=border)
    slide_art(fid, x, y, 300, kind)
    text(fid, x + 14, y + 184, title, 15, T1, weight=600)
    cx = x + 14
    if tag:
        cx += chip(fid, cx, y + 214, "V" + tag, ACCENT, ACCENT_TILE) + 8
    if main:
        cx += chip(fid, cx, y + 214, "主索引", ACCENT, ACCENT_TILE) + 8
    if hist:
        chip(fid, cx, y + 214, "历史", T2, BTN2)
    text(fid, x + 14, y + 244, meta, 11, T3)

def screen_overview(ox):
    fid = frame(ox, "索引总览")
    topbar(fid, "索引页")
    folder_panel(fid, "公司介绍", primary="＋ 新建索引")
    text(fid, 244, 66, "公司介绍", 22, T1, weight=600)
    overview_card(fid, 244, 110, "Redwood 来访介绍", "1.0.0",
                  "超级管理员 · 2026-11-15", "cover", ACCENT, main=True)
    overview_card(fid, 560, 110, "公司介绍 2025", "3.2.0",
                  "超级管理员 · 2026-10-30", "table", BORDER, hist=True)
    return fid

# ---------------- 索引详情 ----------------
def entry_card(fid, x, y, num, name, ver, pages, locked, strip):
    W = 1172
    rect(fid, x, y, W, 196, CARD, r=0, stroke=BORDER)
    text(fid, x + 16, y + 10, f"#{num}", 12, T2)
    kinds = ["cover", "table", "smart", "chart", "table", "cover"]
    slide_art(fid, x + 16, y + 30, 232, kinds[(num - 1) % len(kinds)])
    for i in range(1, pages):
        col, row = (i - 1) % 3, (i - 1) // 3
        slide_art(fid, x + 260 + col * 92, y + 30 + row * 56, 86, kinds[i % len(kinds)])
    # (A: no page-count caption on entry cards — James 2026-11-15)
    ix = x + 560
    text(fid, ix, y + 14, name, 15, T1, weight=600)
    text(fid, ix + 17 * len(name) + 12, y + 17, f"标准 · en · v{ver}", 12, T2)
    # top-right cluster: chip / lock toggle / 选页 / ↑ / ↓ / ×
    ex_ = x + W - 16
    text(fid, ex_ - 12, y + 10, "×", 17, RED)
    text(fid, ex_ - 44, y + 15, "↓", 13, T1)
    text(fid, ex_ - 72, y + 15, "↑", 13, T1)
    text(fid, ex_ - 116, y + 16, "选页", 12, T1)
    if locked:
        text(fid, ex_ - 190, y + 16, "解除锁定", 12, GREEN)
        chip(fid, ex_ - 268, y + 13, "版本锁定", ORANGE, ORANGE_BG)
    else:
        text(fid, ex_ - 202, y + 16, "锁定当前版本", 12, T1)
        chip(fid, ex_ - 280, y + 13, "跟随最新", GREEN, GREEN_BG)
    # quick-switch strip
    sx = ix
    for v in strip:
        rect(fid, sx, y + 108, 54, 30, "#FFFFFF", r=3)
        rect(fid, sx + 4, y + 112, 22, 3, "#1F3864")
        rect(fid, sx + 4, y + 118, 34, 2, "#C9D4E8")
        text(fid, sx + 2, y + 141, "v" + v, 10, ACCENT if v == ver else T2)
        sx += 66
    if len(strip) > 1:
        text(fid, sx + 2, y + 116, "更多", 11, T1)

def insert_row(fid, x, y):
    text(fid, x + 540, y + 4, "＋ 插入条目", 12, T2)

def screen_detail(ox):
    fid = frame(ox, "索引详情")
    topbar(fid, "索引页")
    folder_panel(fid, "公司介绍", primary="＋ 新建索引")
    X = 244
    text(fid, X, 62, "‹ 返回总览", 12, T2)
    text(fid, X, 88, "Redwood 来访介绍", 19, T1, weight=600)
    text(fid, X + 190, 93, "· 超级管理员", 12, T2)
    text(fid, X, 128, "分支", 12, T2)
    rect(fid, X + 32, 122, 90, 26, FIELD, r=6); text(fid, X + 44, 128, "main", 12, T1)
    text(fid, X + 138, 128, "版本 V1.0.0", 12, T2)
    rect(fid, X + 226, 120, 200, 30, FIELD, r=8)
    rect(fid, X + 438, 120, 84, 30, ACCENT, r=15)
    text(fid, X + 452, 127, "发布版本", 12, "#FFFFFF")
    text(fid, X, 162, "添加备注", 12, T2)
    text(fid, X + 76, 162, "导出（个人版）", 12, T1)
    text(fid, X + 190, 162, "导出（公用版）", 12, T1)
    text(fid, X + 480, 162, "拉分支", 12, T1)
    text(fid, X + 544, 162, "推送到演示厅", 12, T1)
    entry_card(fid, X, 190, 1, "开场部分", "1.0.0", 6, False, ["1.0.0"])
    insert_row(fid, X, 392)
    entry_card(fid, X, 424, 2, "技术部分", "1.1.0", 5, False, ["1.1.0", "1.0.0"])
    insert_row(fid, X, 626)
    entry_card(fid, X, 658, 3, "收尾部分", "1.0.0", 5, False, ["1.0.0"])
    return fid

# ---------------- 设置 ----------------
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

# ---------------- 演示厅整稿（H） ----------------
ICON_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..",
                        "crates", "qimu-client", "assets", "icons")

def icon(fid, x, y, name):
    """Place an IconPark PNG (the same asset the client embeds)."""
    return ex("image.place", {"parent": fid, "x": x, "y": y, "width": 20,
                              "height": 20, "name": "icon-" + name,
                              "path": os.path.join(ICON_DIR, name + ".png")})

def screen_deck(ox):
    fid = frame(ox, "演示厅整稿")
    topbar(fid, "演示厅")
    showroom_panel(fid)
    X = 244
    # James 2026-11-15: no "‹ 返回" text, no text actions — the five
    # actions are IconPark icons at the top-right (back in red). The
    # 缓存到本地 action exists only as the download icon.
    text(fid, X, 88, "Redwood 来访介绍", 19, T1, weight=600)
    text(fid, X + 190, 93, "来源索引 Redwood 来访介绍 · 超级管理员", 12, T2)
    for i, name in enumerate(["play", "share", "export", "download", "back"]):
        icon(fid, 1256 + i * 34, 90, name)
    kinds = ["cover", "table", "smart", "chart", "table", "cover",
             "smart", "table", "chart", "cover", "table", "smart",
             "table", "chart", "cover", "table"]
    for n, k in enumerate(kinds):
        cx = X + (n % 5) * 232
        cy = 156 + (n // 5) * 168
        slide_art(fid, cx, cy, 216, k)
        text(fid, cx, cy + 128, f"第 {n + 1} 页", 11, T3)
    return fid

# ---------------- 设置 · 服务器 ----------------
def conn_card(fid, x, y, w, name, url_user, status, status_color, active):
    rect(fid, x, y, w, 64, CARD, r=10, stroke=BORDER)
    text(fid, x + 14, y + 10, name, 14, T1, weight=600)
    if active:
        text(fid, x + 14 + 15 * len(name) + 8, y + 13, "当前", 11, ACCENT)
    text(fid, x + 14, y + 38, url_user, 12, T2)
    text(fid, x + 14 + 11 * len(url_user) + 16, y + 38, status, 12, status_color)
    bx = x + w - 14
    for label in ["删除", "断开", "设为当前"]:
        tw = 13 * len(label) + 16
        bx -= tw
        rect(fid, bx, y + 12, tw, 26, BTN2, r=8)
        text(fid, bx + 8, y + 17, label, 12, T1)
        bx -= 8

def screen_settings_servers(ox):
    fid = frame(ox, "设置服务器")
    topbar(fid, "设置")
    rect(fid, 0, 53, 208, 847, PANEL)
    items = ["服务器", "个人资料", "组织管理", "内容组与授权", "公司公共资料",
             "内容语言", "AI 翻译", "离线缓存策略"]
    for i, name in enumerate(items):
        if i == 0: rect(fid, 8, 65 + i * 40, 192, 34, SEL, r=8)
        text(fid, 20, 72 + i * 40, name, 13, T1 if i == 0 else T2)
    X = 232
    text(fid, X, 68, "服务器", 19, T1, weight=600)
    text(fid, X, 108, "已保存的服务器", 14, T1, weight=600)
    conn_card(fid, X, 132, 900, "192.168.1.10", "http://192.168.1.10:8730 · admin",
              "已连接", GREEN, True)
    conn_card(fid, X, 206, 900, "127.0.0.1", "http://127.0.0.1:8730 · admin",
              "连接失败", RED, False)
    text(fid, X, 292, "添加服务器", 14, T1, weight=600)
    for i, (lb, val) in enumerate([("服务器地址", "http://192.168.1.20:8730"),
                                   ("账号", "admin"), ("密码", "••••••••")]):
        y = 322 + i * 44
        text(fid, X, y + 7, lb, 13, T2)
        rect(fid, X + 96, y, 380, 32, FIELD, r=8)
        text(fid, X + 108, y + 8, val, 13, T1)
    rect(fid, X + 96, 322 + 3 * 44, 120, 32, ACCENT, r=16)
    text(fid, X + 124, 322 + 3 * 44 + 8, "登录并添加", 12.5, "#FFFFFF", weight=600)
    y = 322 + 3 * 44 + 52
    text(fid, X, y, "局域网发现", 14, T1, weight=600)
    rect(fid, X + 96, y - 6, 150, 30, BTN2, r=8)
    text(fid, X + 112, y, "搜索局域网服务器", 12, T1)
    for j, (nm, url) in enumerate([("QIMU-HOST", "http://192.168.1.10:8730 · v0.1.0"),
                                   ("NAS-01", "http://192.168.1.30:8730 · v0.1.0")]):
        cy = y + 36 + j * 56
        rect(fid, X, cy, 900, 48, CARD, r=10, stroke=BORDER)
        text(fid, X + 14, cy + 15, nm, 13, T1)
        text(fid, X + 130, cy + 16, url, 12, T2)
        rect(fid, X + 900 - 110, cy + 10, 96, 28, BTN2, r=8)
        text(fid, X + 900 - 96, cy + 15, "添加并登录", 12, T1)
    y2 = y + 36 + 2 * 56 + 18
    rect(fid, X, y2, 900, 1, "#2A2A2A")
    text(fid, X, y2 + 14, "缓存路径（本机设置）", 14, T1, weight=600)
    text(fid, X, y2 + 42, "当前生效：D:\\Program Files\\启幕\\cache · 3 条缓存 · 12.4 MB", 12, T2)
    rect(fid, X, y2 + 66, 460, 32, FIELD, r=8)
    text(fid, X + 12, y2 + 74, "D:\\Program Files\\启幕\\cache", 12.5, T1)
    rect(fid, X + 472, y2 + 66, 84, 32, BTN2, r=8); text(fid, X + 488, y2 + 73, "保存路径", 12, T1)
    rect(fid, X + 564, y2 + 66, 84, 32, BTN2, r=8); text(fid, X + 580, y2 + 73, "恢复默认", 12, T1)
    return fid

# ---------------- 内容页选页模式（B/C） ----------------
def screen_pick(ox):
    fid = frame(ox, "内容页选页模式")
    topbar(fid, "内容页")
    folder_panel(fid, "公司介绍")
    X = 244
    # context bar: accent-tinted strip with the mode's confirm/cancel
    rect(fid, X, 62, 1172, 40, ACCENT_TILE, r=8)
    text(fid, X + 14, 74, "为索引「Redwood 来访介绍」选页 · 技术部分", 13, T1, weight=600)
    rect(fid, X + 1172 - 196, 68, 84, 28, ACCENT, r=14)
    text(fid, X + 1172 - 172, 74, "确认选页", 12, "#FFFFFF", weight=600)
    rect(fid, X + 1172 - 100, 68, 84, 28, BTN2, r=14)
    text(fid, X + 1172 - 66, 74, "取消", 12, T1)
    text(fid, X, 122, "技术部分", 17, T1, weight=600)
    text(fid, X + 86, 126, "标准 · en · v1.1.0（当前解析版本）", 12, T2)
    kinds = ["table", "cover", "smart", "chart", "table"]
    checked = [True, True, False, False, False]
    for n, k in enumerate(kinds):
        cx = X + (n % 4) * 232
        cy = 156 + (n // 4) * 190
        slide_art(fid, cx, cy, 216, k)
        if checked[n]:
            rect(fid, cx, cy + 134, 16, 16, ACCENT, r=4)
            text(fid, cx + 3, cy + 134, "✓", 11, "#FFFFFF")
        else:
            rect(fid, cx, cy + 134, 16, 16, BG, r=4, stroke=BORDER)
        text(fid, cx + 24, cy + 135, f"第 {n + 1} 页", 12, T1 if checked[n] else T2)
    return fid

if __name__ == "__main__":
    ex("file.new")
    boards = []
    boards.append(("登录.png", screen_login(0)))
    boards.append(("演示厅.png", screen_showroom(1500)))
    boards.append(("内容页.png", screen_content(3000)))
    boards.append(("索引总览.png", screen_overview(4500)))
    boards.append(("索引详情.png", screen_detail(6000)))
    boards.append(("设置.png", screen_settings(7500)))
    boards.append(("演示厅整稿.png", screen_deck(9000)))
    boards.append(("设置服务器.png", screen_settings_servers(10500)))
    boards.append(("内容页选页模式.png", screen_pick(12000)))
    for name, fid in boards:
        r = call("ui.render", {"path": f"/home/hatch/workspace/qimu/design/{name}",
                               "scale": 1, "ids": [fid]})
        print(name, "rendered")
    r = ex("file.saveAs", {"path": "/home/hatch/workspace/qimu/design/qimu-ui.figcraft"})
    print("saved", r)
