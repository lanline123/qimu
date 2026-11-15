#!/usr/bin/env python3
"""E2E: showroom sharing (§3.17) — direct / department / custom-group
shares, inbox, accept into library, item-level export access, and the
negative cases (no share → no access; non-member → empty group space)."""
import json, urllib.request, urllib.error

BASE = "http://127.0.0.1:8730"

def call(method, path, token=None, body=None, expect=None):
    req = urllib.request.Request(BASE + path, method=method)
    if token:
        req.add_header("Authorization", "Bearer " + token)
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, data=data, timeout=120) as r:
            out = json.loads(r.read().decode())
            code = r.status
    except urllib.error.HTTPError as e:
        out = json.loads(e.read().decode() or "{}")
        code = e.code
    if expect is not None:
        assert code == expect, f"{method} {path} -> {code} (want {expect}): {out}"
    return code, out

admin = call("POST", "/v1/auth/login", body={"username": "admin", "password": "admin12345"})[1]["token"]

# org: 技术部 (parent) with 子部门 实验室; 市场部 separate
org = call("GET", "/v1/org", admin)[1]
depts = {d["name"]: d["id"] for d in org["departments"]}
def ensure_dept(name, parent=None):
    if name in depts:
        return depts[name]
    body = {"name": name}
    if parent:
        body["parent_id"] = parent
    d = call("POST", "/v1/org/departments", admin, body)[1]
    depts[name] = d["id"]
    return d["id"]

tech = ensure_dept("技术部")
lab = ensure_dept("实验室", tech)
market = ensure_dept("市场部")

users = {u["username"]: u for u in call("GET", "/v1/org", admin)[1]["users"]}
def ensure_user(username, dept):
    if username in users:
        return users[username]
    u = call("POST", "/v1/org/users", admin, {
        "username": username, "password": "pass12345",
        "display_name": username, "dept_id": dept, "role": "member",
    })[1]
    users[username] = u
    return u

bob = ensure_user("bob", lab)       # in 实验室 ⊂ 技术部
carol = ensure_user("carol", market)
bob_tok = call("POST", "/v1/auth/login", body={"username": "bob", "password": "pass12345"})[1]["token"]
carol_tok = call("POST", "/v1/auth/login", body={"username": "carol", "password": "pass12345"})[1]["token"]

item = call("GET", "/v1/showroom?state=live", admin)[1]["items"][0]
iid = item["id"]
print("showroom item:", item["title"], iid)

# negative: carol cannot export before any share
call("POST", f"/v1/showroom/{iid}/export", carol_tok, {"kind": "company"}, expect=404)
print("negative ok: carol export before share -> 404")

# 1) direct share admin -> bob
sid = call("POST", "/v1/shares", admin, {
    "item_kind": "showroom", "item_id": iid,
    "target_kind": "user", "target_id": bob["id"],
})[1]["share_id"]
inbox = call("GET", "/v1/shares/inbox", bob_tok)[1]["shares"]
assert any(s["id"] == sid for s in inbox), inbox
assert not any(s["id"] == sid for s in call("GET", "/v1/shares/inbox", carol_tok)[1]["shares"])
print("direct share in bob's inbox only")

# accept -> library -> export works for bob
lib = call("POST", f"/v1/shares/{sid}/accept", bob_tok, {"folder": "同事分享"})[1]
entries = call("GET", "/v1/library", bob_tok)[1]["entries"]
assert any(e["id"] == lib["library_id"] and e["folder"] == "同事分享" for e in entries), entries
exp = call("POST", f"/v1/showroom/{iid}/export", bob_tok, {"kind": "company"})[1]
assert exp["page_count"] > 0 and exp["data_base64"]
print("bob accepted into library and exported:", exp["page_count"], "pages")

# 2) department share to 技术部: bob (via 实验室 sub-dept) sees it, carol doesn't
sid2 = call("POST", "/v1/shares", admin, {
    "item_kind": "showroom", "item_id": iid,
    "target_kind": "dept", "target_id": tech,
})[1]["share_id"]
assert any(s["id"] == sid2 for s in call("GET", "/v1/shares/inbox", bob_tok)[1]["shares"])
assert not any(s["id"] == sid2 for s in call("GET", "/v1/shares/inbox", carol_tok)[1]["shares"])
print("dept share reaches sub-department member, not other departments")

# 3) custom group: admin creates 项目组, adds carol; share to group
gid = call("POST", "/v1/custom-groups", admin, {"name": "项目组"})[1]["id"]
call("POST", f"/v1/custom-groups/{gid}/members", admin, {"user_id": carol["id"]})
sid3 = call("POST", "/v1/shares", admin, {
    "item_kind": "showroom", "item_id": iid,
    "target_kind": "group", "target_id": gid,
})[1]["share_id"]
carol_space = call("GET", "/v1/group-space", carol_tok)[1]["spaces"]
assert any(sp["group"]["id"] == gid and any(i["id"] == sid3 for i in sp["items"]) for sp in carol_space), carol_space
bob_space = call("GET", "/v1/group-space", bob_tok)[1]["spaces"]
assert not any(any(i["id"] == sid3 for i in sp["items"]) for sp in bob_space)
# group share also unlocks item-level export for carol
exp2 = call("POST", f"/v1/showroom/{iid}/export", carol_tok, {"kind": "company"})[1]
assert exp2["page_count"] > 0
print("group share: carol sees it in group space + can export; bob sees nothing")

# leaving the group revokes access
call("POST", f"/v1/custom-groups/{gid}/members/remove", admin, {"user_id": carol["id"]})
call("POST", f"/v1/showroom/{iid}/export", carol_tok, {"kind": "company"}, expect=404)
print("after leaving the group, carol's access is revoked")
print("SHARE TESTS ALL GREEN")
