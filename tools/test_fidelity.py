#!/usr/bin/env python3
"""Scenario-13 fidelity gate (partial, automatable part):
for case deck #2 (charts/animation/video) and #2b (real SmartArt):
  import (whole deck as one block) -> index -> export
  then verify the exported package still carries charts + embedded
  workbooks / media / animation timing / diagram parts, page fingerprints
  match the source, and LibreOffice renders both decks page-for-page
  near-identically. Finally: manifest survival through a LibreOffice
  open->save round trip (the §8.6 proxy check)."""
import base64, io, json, subprocess, sys, urllib.request, urllib.error, zipfile
from PIL import Image, ImageChops

BASE = "http://127.0.0.1:8730"
FILES = "/home/hatch/workspace/user/files"

def call(method, path, token=None, body=None):
    req = urllib.request.Request(BASE + path, method=method)
    if token:
        req.add_header("Authorization", "Bearer " + token)
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        req.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(req, data=data, timeout=600) as r:
        return json.loads(r.read().decode())

def render_pngs(pptx_bytes, tag):
    import os
    d = f"/tmp/fidelity-{tag}"
    subprocess.run(["rm", "-rf", d])
    os.makedirs(d)
    open(f"{d}/in.pptx", "wb").write(pptx_bytes)
    subprocess.run(["soffice", f"-env:UserInstallation=file://{d}/lo", "--headless",
                    "--convert-to", "pdf", "--outdir", d, f"{d}/in.pptx"],
                   capture_output=True, timeout=300)
    subprocess.run(["pdftoppm", "-png", "-r", "40", f"{d}/in.pdf", f"{d}/p"],
                   capture_output=True, timeout=120)
    import glob
    return sorted(glob.glob(f"{d}/p-*.png"))

def mean_diff(a, b):
    ia = Image.open(a).convert("RGB")
    ib = Image.open(b).convert("RGB")
    if ia.size != ib.size:
        return 999.0
    diff = ImageChops.difference(ia, ib)
    px = list(diff.getdata())
    return sum(sum(p) for p in px) / (len(px) * 3)

tok = call("POST", "/v1/auth/login", body={"username": "admin", "password": "admin12345"})["token"]
gid = next(g["id"] for g in call("GET", "/v1/groups", tok)["groups"] if g["name"] == "公司介绍")
fid = call("GET", "/v1/folders", tok)["folders"][0]["id"]

def roundtrip(fname, pages, expect_parts, label):
    raw = open(f"{FILES}/{fname}", "rb").read()
    n = pages
    imp = call("POST", "/v1/import", tok, {
        "filename": fname,
        "data_base64": base64.b64encode(raw).decode(),
        "folder_id": fid, "group_id": gid,
        "blocks": [{"name": label, "pages": list(range(n)), "form": "标准", "lang": "zh"}],
    })
    assert imp["slide_count"] == n, imp
    units = call("GET", "/v1/units", tok)["units"]
    unit = next(i for i in units if i["unit"]["name"] == label)
    rend_id = unit["renditions"][0]["rendition"]["id"]
    idx = call("POST", "/v1/indexes", tok, {
        "name": label + " 索引", "group_id": gid, "scope": "company",
        "entries": [{"unit_id": unit["unit"]["id"], "rendition_id": rend_id}],
    })
    exp = call("POST", f"/v1/indexes/{idx['id']}/export", tok, {"branch": "main", "kind": "company"})
    out = base64.b64decode(exp["data_base64"])
    z = zipfile.ZipFile(io.BytesIO(out))
    names = z.namelist()
    for part in expect_parts:
        assert any(part in x for x in names), f"{label}: missing part like {part}"
    # animation timing must survive on the animation slide of deck #2
    if label == "案例2图表":
        s6 = [x for x in names if x.startswith("ppt/slides/") and x.endswith(".xml")]
        joined = b"".join(z.read(x) for x in s6).decode(errors="ignore")
        assert "<p:timing>" in joined, "animation timing lost"
        assert "<p:transition" in joined, "transition lost"
    # render comparison
    src_pngs = render_pngs(raw, label + "-src")
    out_pngs = render_pngs(out, label + "-out")
    assert len(src_pngs) == len(out_pngs) == n, (len(src_pngs), len(out_pngs))
    diffs = [round(mean_diff(a, b), 2) for a, b in zip(src_pngs, out_pngs)]
    print(f"{label}: parts OK, render pages={n}, mean abs diff per page = {diffs}")
    assert max(diffs) < 12.0, f"render diverged: {diffs}"
    return out

out2 = roundtrip("Case2_Charts_Animation.pptx", 7,
                 ["ppt/charts/", "ppt/embeddings/", "ppt/media/", ".mp4"],
                 "案例2图表")
roundtrip("Case2b_SmartArt.pptx", 1, ["ppt/diagrams/data", "ppt/diagrams/layout"], "案例2SmartArt")

# --- manifest survival through LibreOffice open->save:
# LO strips the custom property AND the mirror part, but preserves core
# keywords (normalizing "qimu:deck_x" to "qimu deck_x"); the CLI must
# recover the full manifest from the server's export registry by deck id.
import os, re
d = "/tmp/fidelity-manifest"
subprocess.run(["rm", "-rf", d]); os.makedirs(f"{d}/src"); os.makedirs(f"{d}/out")
open(f"{d}/src/in.pptx", "wb").write(out2)
r = subprocess.run(["soffice", f"-env:UserInstallation=file://{d}/lo", "--headless",
                    "--convert-to", "pptx", "--outdir", f"{d}/out", f"{d}/src/in.pptx"],
                   capture_output=True, timeout=300)
assert r.returncode == 0, r.stderr.decode()
conv = f"{d}/out/in.pptx"
z = zipfile.ZipFile(conv)
names = z.namelist()
print("after LO round trip: mirror part =", "qimu/manifest.json" in names,
      "| custom prop =", "qimu_manifest" in (
          z.read("docProps/custom.xml").decode(errors="ignore")
          if "docProps/custom.xml" in names else ""))
core = z.read("docProps/core.xml").decode(errors="ignore")
m = re.search(r"qimu[: ]+(deck_[a-z0-9]+)", core)
assert m, "deck-id token lost in core keywords"
print("deck-id token survived (normalized form):", m.group(1))
chk = subprocess.run(["/home/hatch/workspace/qimu/target/debug/qimu", "deck", "check", conv],
                     capture_output=True, text=True, timeout=120)
assert chk.returncode == 0, chk.stderr + chk.stdout
res = json.loads(chk.stdout)
assert res["deck_id"] == m.group(1) and res["status"] == "ok", res
print("deck check recovered manifest from server registry:", res["deck_id"], "up_to_date =", res["up_to_date"])
print("FIDELITY TESTS DONE")
