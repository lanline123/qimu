#!/usr/bin/env python3
"""E2E test for §3.14 personal binding on export.
Run against the dev server (seeded). Creates a template unit whose first
slide reads 汇报人：{{name}} · {{phone}} · {{email}} · {{company_website}},
then checks personal / company / missing-field export behaviour."""
import base64, io, json, re, sys, urllib.request, urllib.error, zipfile

BASE = "http://127.0.0.1:8730"

def call(method, path, token=None, body=None, expect_error=False):
    req = urllib.request.Request(BASE + path, method=method)
    if token:
        req.add_header("Authorization", "Bearer " + token)
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, data=data, timeout=300) as r:
            return json.loads(r.read().decode())
    except urllib.error.HTTPError as e:
        payload = json.loads(e.read().decode())
        if expect_error:
            return payload
        raise SystemExit(f"{method} {path} -> {e.code}: {payload}")

tok = call("POST", "/v1/auth/login", body={"username": "admin", "password": "admin12345"})["token"]
gid = next(g["id"] for g in call("GET", "/v1/groups", tok)["groups"] if g["name"] == "公司介绍")

# profile + company data
call("PUT", "/v1/me/profile", tok, {"fields": {"phone": "13800001111", "email": "admin@example.com", "title": "技术总监"}})
call("PUT", "/v1/company-profile", tok, {"fields": {"website": "www.beixing.example", "phone": "0532-88888888"}})

# doctor the 开场部分 fragment: slide1 first text run -> template line
units = call("GET", "/v1/units", tok)["units"]
first = next(i for i in units if i["unit"]["name"] == "开场部分")
ver_id = first["renditions"][0]["latest"]["id"]
frag = base64.b64decode(call("GET", f"/v1/versions/{ver_id}/fragment", tok)["data_base64"])
zin = zipfile.ZipFile(io.BytesIO(frag))
slide_name = "ppt/slides/slide1.xml"
xml = zin.read(slide_name).decode()
m = re.search(r"<a:t>(.*?)</a:t>", xml, re.S)
xml = xml[:m.start(1)] + "汇报人：{{name}} · {{phone}} · {{email}} · {{company_website}}" + xml[m.end(1):]
buf = io.BytesIO()
with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as zout:
    for item in zin.infolist():
        data = zin.read(item.filename)
        if item.filename == slide_name:
            data = xml.encode()
        zout.writestr(item, data)
template_frag = base64.b64encode(buf.getvalue()).decode()

unit = call("POST", "/v1/units", tok, {"name": "封面模板", "group_id": gid})
call("POST", f"/v1/units/{unit['id']}/publish", tok, {
    "data_base64": template_frag, "form": "标准", "lang": "zh", "bump": "minor", "note": "模板",
})
detail = call("GET", f"/v1/units/{unit['id']}", tok)
rend_id = detail["renditions"][0]["rendition"]["id"]
idx = call("POST", "/v1/indexes", tok, {
    "name": "模板导出测试", "group_id": gid, "scope": "company",
    "entries": [{"unit_id": unit["id"], "rendition_id": rend_id}],
})

def slide1_text(export):
    z = zipfile.ZipFile(io.BytesIO(base64.b64decode(export["data_base64"])))
    xml = z.read("ppt/slides/slide1.xml").decode()
    return "".join(re.findall(r"<a:t>(.*?)</a:t>", xml, re.S))

exp = call("POST", f"/v1/indexes/{idx['id']}/export", tok, {"branch": "main", "kind": "personal"})
t = slide1_text(exp)
assert "超级管理员" in t and "13800001111" in t and "admin@example.com" in t and "www.beixing.example" in t, t
assert "{{" not in t, t
print("personal export OK:", t[:80])

exp = call("POST", f"/v1/indexes/{idx['id']}/export", tok, {"branch": "main", "kind": "company"})
t = slide1_text(exp)
assert "超级管理员" not in t and "13800001111" not in t and "admin@example.com" not in t, t
assert "www.beixing.example" in t and "{{" not in t, t
print("company export OK:", t[:80])

call("PUT", "/v1/me/profile", tok, {"fields": {"email": ""}})
err = call("POST", f"/v1/indexes/{idx['id']}/export", tok, {"branch": "main", "kind": "personal"}, expect_error=True)
assert "email" in err.get("error", ""), err
print("missing-field block OK:", err["error"])
print("PERSONALIZE E2E OK")
