#!/usr/bin/env python3
"""E2E: AI translation pipeline (§3.13) with the stub provider.
Glossary substitution applies, the output is a DRAFT rendition in the
target language, page bindings record the ai pairing, and the speaker
scripts are translated along."""
import json, urllib.request

BASE = "http://127.0.0.1:8730"

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

ai = call("GET", "/v1/settings/ai", tok)
assert ai["provider"] == "stub", ai
print("provider:", ai["provider"])

call("PUT", "/v1/settings/glossary", tok, {"pairs": [{"src": "华东", "dst": "EastChina"}]})
g = call("GET", "/v1/settings/glossary", tok)
assert g["pairs"][0]["dst"] == "EastChina", g
print("glossary saved")

units = call("GET", "/v1/units", tok)["units"]
unit = next(u for u in units if u["unit"]["name"] == "案例2图表")
uid = unit["unit"]["id"]
detail = call("GET", f"/v1/units/{uid}", tok)
src_rend = next(r for r in detail["renditions"] if r["rendition"]["lang"] == "zh")
src_ver = src_rend["versions"][0]
assert src_ver["scripts"], "source version should carry scripts"
print("source:", src_rend["rendition"]["lang"], src_ver["semver"], "scripts:", len(src_ver["scripts"]))

res = call("POST", f"/v1/units/{uid}/translate", tok, {
    "version_id": src_ver["id"], "target_lang": "en",
})
nv = res["version"]
assert nv["status"] == "draft", nv
assert nv["semver"] == "1.0.0", nv
assert res["translated_runs"] > 0, res
print("draft created:", nv["id"], "runs:", res["translated_runs"], "| note:", nv["note"][:40])

detail2 = call("GET", f"/v1/units/{uid}", tok)
en_rend = next(r for r in detail2["renditions"] if r["rendition"]["lang"] == "en")
en_ver = next(v for v in en_rend["versions"] if v["id"] == nv["id"])
# scripts translated along (stub prefixes + glossary)
assert en_ver["scripts"][0].startswith("[EN]"), en_ver["scripts"][0]
print("scripts translated:", en_ver["scripts"][0][:30])

# slide texts translated: searchable text of the draft contains markers
frag = call("GET", f"/v1/versions/{nv['id']}/fragment", tok)
import base64, io, zipfile
z = zipfile.ZipFile(io.BytesIO(base64.b64decode(frag["data_base64"])))
slide1 = z.read([n for n in z.namelist() if n.startswith("ppt/slides/slide")][0]).decode()
assert "[EN]" in slide1, slide1[:400]
# glossary applies to translated text (the 华东 term lives in the page-2
# script; chart-internal cached labels are outside run-level translation)
assert "EastChina" in en_ver["scripts"][1], en_ver["scripts"][1]
print("slide runs translated; glossary applied in scripts:", en_ver["scripts"][1][:40])

bindings = call("GET", f"/v1/units/{uid}/page-bindings", tok)["bindings"]
ai_bindings = [b for b in bindings if b["kind"] == "ai" and b["to_version_id"] == nv["id"]]
assert len(ai_bindings) == src_ver["page_count"], (len(ai_bindings), src_ver["page_count"])
print("ai page bindings recorded:", len(ai_bindings))
print("TRANSLATE TESTS ALL GREEN")
