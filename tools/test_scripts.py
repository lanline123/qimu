#!/usr/bin/env python3
"""E2E: speaker scripts (§3.16). Publish a fresh version from the case-2
deck (its notes become the initial scripts), edit one page's script via
the API (semver must NOT change), then export with include_notes and
verify the edited script lands in the exported deck's notes page —
while an export without the flag keeps the original notes."""
import base64, io, json, urllib.request, urllib.error
from pptx import Presentation

BASE = "http://127.0.0.1:8730"
DECK = "/home/hatch/workspace/user/files/Case2_Charts_Animation.pptx"

def call(method, path, token=None, body=None):
    req = urllib.request.Request(BASE + path, method=method)
    if token:
        req.add_header("Authorization", "Bearer " + token)
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        req.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(req, data=data, timeout=300) as r:
        return json.loads(r.read().decode())

tok = call("POST", "/v1/auth/login", body={"username": "admin", "password": "admin12345"})["token"]
units = call("GET", "/v1/units", tok)["units"]
unit = next(u for u in units if u["unit"]["name"] == "案例2图表")
uid = unit["unit"]["id"]
raw = open(DECK, "rb").read()

pub = call("POST", f"/v1/units/{uid}/publish", tok, {
    "data_base64": base64.b64encode(raw).decode(),
    "form": "标准", "lang": "zh", "bump": "minor", "note": "讲稿测试版",
})
ver = pub  # publish returns the VersionRec directly
vid = ver["id"]
print("published", ver["semver"], vid)
scripts = ver["scripts"]
assert len(scripts) == 7, scripts
assert "开场备注" in scripts[0], scripts[0]
assert "柱状图备注" in scripts[1], scripts[1]
print("initial scripts extracted from notes:", [s[:12] for s in scripts])

# edit page 2's script; version must not change
scripts[1] = "改过的讲稿：这一页重点讲华东大区。\n第二行补充说明。"
call("PUT", f"/v1/versions/{vid}/scripts", tok, {"scripts": scripts})
back = call("GET", f"/v1/versions/{vid}", tok)["version"]
assert back["semver"] == ver["semver"], (back["semver"], ver["semver"])
assert back["scripts"][1].startswith("改过的讲稿"), back["scripts"][1]
print("script edited in place, semver unchanged:", back["semver"])

idx = next(i for i in call("GET", "/v1/indexes", tok)["indexes"] if i["name"] == "案例2图表 索引")

def notes_of(deck_bytes, page):
    prs = Presentation(io.BytesIO(deck_bytes))
    slide = prs.slides[page]
    return slide.notes_slide.notes_text_frame.text

exp = call("POST", f"/v1/indexes/{idx['id']}/export", tok, {"branch": "main", "kind": "company", "include_notes": True})
out = base64.b64decode(exp["data_base64"])
n1 = notes_of(out, 1)
assert "改过的讲稿" in n1 and "第二行补充说明" in n1, n1
n0 = notes_of(out, 0)
assert "开场备注" in n0, n0
print("export with notes: edited script written back to notes pages")

exp2 = call("POST", f"/v1/indexes/{idx['id']}/export", tok, {"branch": "main", "kind": "company"})
out2 = base64.b64decode(exp2["data_base64"])
n1b = notes_of(out2, 1)
assert "改过的讲稿" not in n1b and "柱状图备注" in n1b, n1b
print("export without flag: original notes untouched")
print("SCRIPT TESTS ALL GREEN")
