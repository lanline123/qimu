#!/usr/bin/env python3
"""Dev seed: import the Redwood deck as 3 blocks, build an index, tag
V1.0.0, publish a 1.1.0 to 技术部分， and put the index on the showroom.
Idempotent-ish: run against a FRESH data dir."""
import base64, json, sys, urllib.request

BASE = "http://127.0.0.1:8730"
DECK = "/home/hatch/workspace/user/files/Redwood_Visit_Revised_4tph_0_6vcr.pptx"

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
groups = call("GET", "/v1/groups", tok)["groups"]
gid = next(g["id"] for g in groups if g["name"] == "公司介绍")
folders = call("GET", "/v1/folders", tok)["folders"]
fid = next((f["id"] for f in folders if f["name"] == "公司介绍"), folders[0]["id"])

deck = open(DECK, "rb").read()
blocks = [
    {"name": "开场部分", "pages": list(range(0, 6)), "form": "标准", "lang": "en"},
    {"name": "技术部分", "pages": list(range(6, 12)), "form": "标准", "lang": "en"},
    {"name": "收尾部分", "pages": list(range(12, 17)), "form": "标准", "lang": "en"},
]
imp = call("POST", "/v1/import", tok, {
    "filename": "Redwood_Visit_Revised_4tph_0_6vcr.pptx",
    "data_base64": base64.b64encode(deck).decode(),
    "folder_id": fid, "group_id": gid, "blocks": blocks,
})
print("import:", imp.get("slide_count"), "pages")

units = call("GET", "/v1/units", tok)["units"]
entries, by_name = [], {}
for item in units:
    u = item["unit"]
    by_name[u["name"]] = item
    for r in item["renditions"]:
        entries.append({"unit_id": u["id"], "rendition_id": r["rendition"]["id"]})
idx = call("POST", "/v1/indexes", tok, {
    "name": "Redwood 来访介绍", "group_id": gid, "scope": "company", "entries": entries,
})
print("index:", idx["id"])
call("POST", f"/v1/indexes/{idx['id']}/tags", tok, {"branch": "main", "name": "1.0.0"})
print("tagged V1.0.0")

tech = by_name["技术部分"]["unit"]["id"]
tail_ver = by_name["收尾部分"]["renditions"][0]["latest"]["id"]
frag = call("GET", f"/v1/versions/{tail_ver}/fragment", tok)
pub = call("POST", f"/v1/units/{tech}/publish", tok, {
    "data_base64": frag["data_base64"], "form": "标准", "lang": "en",
    "bump": "minor", "note": "技术参数更新",
})
print("published 技术部分 v" + pub["semver"])

sr = call("POST", "/v1/showroom", tok, {
    "title": "Redwood 来访介绍", "intro": "电池回收技术交流 · 17 页", "source_index_id": idx["id"],
})
call("POST", f"/v1/showroom/{sr['id']}/review", tok, {"state": "live"})
print("showroom live:", sr["id"])
print("SEED OK")
