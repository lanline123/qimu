#!/usr/bin/env python3
"""E2E test for §3.10 entry-level three-way branch merge."""
import copy, json, urllib.request, urllib.error

BASE = "http://127.0.0.1:8730"
import time
RUN = str(int(time.time()))[-5:]  # unique branch names per run (idempotent reruns)

def call(method, path, token=None, body=None, expect_error=False):
    req = urllib.request.Request(BASE + path, method=method)
    if token:
        req.add_header("Authorization", "Bearer " + token)
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, data=data, timeout=120) as r:
            return json.loads(r.read().decode())
    except urllib.error.HTTPError as e:
        payload = json.loads(e.read().decode())
        if expect_error:
            return payload
        raise SystemExit(f"{method} {path} -> {e.code}: {payload}")

tok = call("POST", "/v1/auth/login", body={"username": "admin", "password": "admin12345"})["token"]
idxs = call("GET", "/v1/indexes", tok)["indexes"]
idx = next(i["id"] for i in idxs if i["name"] == "Redwood 来访介绍")
detail = call("GET", f"/v1/indexes/{idx}", tok)
base_entries = detail["head"]["entries"]
tech_pos = next(n for n, e in enumerate(base_entries)
                if detail["resolved_entries"][n]["unit_name"] == "技术部分")

units = call("GET", "/v1/units", tok)["units"]
tech = next(i for i in units if i["unit"]["name"] == "技术部分")
tech_detail = call("GET", f"/v1/units/{tech['unit']['id']}", tok)
versions = tech_detail["renditions"][0]["versions"]
v100 = next(v["id"] for v in versions if v["semver"] == "1.0.0")
v110 = next(v["id"] for v in versions if v["semver"] == "1.1.0")

def lock(entries, version_id, semver):
    es = copy.deepcopy(entries)
    es[tech_pos]["policy"] = {"locked": {"version_id": version_id}}
    es[tech_pos]["locked_semver"] = semver
    return es

# --- round 1: one-sided change merges cleanly
call("POST", f"/v1/indexes/{idx}/branches", tok, {"name": f"feature-{RUN}", "from": "main"})
call("POST", f"/v1/indexes/{idx}/commits", tok, {
    "branch": f"feature-{RUN}", "message": "锁定技术部分到 1.0.0", "entries": lock(base_entries, v100, "1.0.0"),
})
p = call("POST", f"/v1/indexes/{idx}/merge/preview", tok, {"from": f"feature-{RUN}", "into": "main"})
assert p["clean"] and not p["conflicts"], p
assert p["merged"][tech_pos]["policy"] == {"locked": {"version_id": v100}}, p["merged"][tech_pos]
call("POST", f"/v1/indexes/{idx}/merge", tok, {"from": f"feature-{RUN}", "into": "main"})
d2 = call("GET", f"/v1/indexes/{idx}", tok)
assert d2["head"]["entries"][tech_pos]["policy"] == {"locked": {"version_id": v100}}
print("round 1 clean merge OK")

# --- round 2: both sides change the same entry -> conflict, adjudicate
main_entries = d2["head"]["entries"]
call("POST", f"/v1/indexes/{idx}/branches", tok, {"name": f"feature2-{RUN}", "from": "main"})
call("POST", f"/v1/indexes/{idx}/commits", tok, {
    "branch": f"feature2-{RUN}", "message": "改锁 1.1.0", "entries": lock(main_entries, v110, "1.1.0"),
})
es = copy.deepcopy(main_entries)
es[tech_pos]["excerpt_slots"] = ["0"]
call("POST", f"/v1/indexes/{idx}/commits", tok, {
    "branch": "main", "message": "技术部分只用第一页", "entries": es,
})
p = call("POST", f"/v1/indexes/{idx}/merge/preview", tok, {"from": f"feature2-{RUN}", "into": "main"})
assert not p["clean"] and len(p["conflicts"]) == 1, p
key = p["conflicts"][0]["key"]
print("conflict key:", key)
err = call("POST", f"/v1/indexes/{idx}/merge", tok, {"from": f"feature2-{RUN}", "into": "main"}, expect_error=True)
assert "冲突" in err.get("error", ""), err
print("unresolved merge blocked OK:", err["error"][:60])
call("POST", f"/v1/indexes/{idx}/merge", tok, {
    "from": f"feature2-{RUN}", "into": "main", "resolutions": {key: "theirs"},
})
d3 = call("GET", f"/v1/indexes/{idx}", tok)
assert d3["head"]["entries"][tech_pos]["policy"] == {"locked": {"version_id": v110}}, d3["head"]["entries"][tech_pos]
print("adjudicated merge OK")
print("MERGE E2E OK")
