#!/usr/bin/env python3
"""What each server writes to the database, step by step, for the same client scenario.

One server at a time on :8065, started from the same data each time (dev/bench/sqldiff.sh restores a snapshot), with
pg_stat_statements loaded. Before every step the statistics are reset; after it (and a pause for the work done after
the answer) the writes are read back as (operation, table, rows). SQL texts differ between Go and Rust, so the writes
are compared by what they touch, not by their text. Also kept per step: the status, the answer's top-level keys and
the websocket events three observers received.

    sqldiff.py run <impl> <out.json>         against the server on :8065
    sqldiff.py compare <go.json> <rust.json> the differences, step by step

Stdlib and websocket-client only.
"""

import json
import os
import re
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request

BASE = os.environ.get("SQLDIFF_URL", "http://localhost:8065")
PG = os.environ.get("SQLDIFF_PG", "bench-db")
PASSWORD = "Bench-pass-123"
# Writes that come from timers rather than from the step (Go's job scheduler, cluster and status flushes).
BACKGROUND = {"jobs", "systems", "clusterdiscovery", "pluginkeyvaluestore"}


def psql(sql):
    return subprocess.check_output(
        ["docker", "exec", PG, "psql", "-U", "mmuser", "-d", "mattermost", "-AtF\t", "-c", sql]).decode()


def call(token, method, path, body=None, raw=None, ctype=None):
    data = raw if raw is not None else (None if body is None else json.dumps(body).encode())
    req = urllib.request.Request(BASE + "/api/v4" + path, data=data, method=method)
    if token:
        req.add_header("Authorization", "Bearer " + token)
    if data is not None:
        req.add_header("Content-Type", ctype or "application/json")
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            out, status, headers = r.read(), r.status, r.headers
    except urllib.error.HTTPError as e:
        out, status, headers = e.read(), e.code, e.headers
    try:
        val = json.loads(out) if out[:1] in (b"{", b"[") else None
    except ValueError:
        val = None
    return status, val, headers


def login(user, password=PASSWORD):
    s, v, h = call(None, "POST", "/users/login", {"login_id": user, "password": password})
    assert s == 200, (user, s, v)
    return h["Token"], v["id"]


def keys(v):
    if isinstance(v, dict):
        return v.get("id") if "status_code" in v else sorted(v.keys())
    if isinstance(v, list):
        return "array" if not v or not isinstance(v[0], dict) else ["array of", sorted(v[0].keys())]
    return type(v).__name__


WRITE = re.compile(r"\b(INSERT\s+INTO|UPDATE|DELETE\s+FROM)\s+\"?([A-Za-z_]+)", re.I)


def writes():
    """(operation, table) -> [calls, rows] for every statement that writes, since the last reset."""
    out = {}
    for line in psql("SELECT calls, rows, query FROM pg_stat_statements WHERE query NOT ILIKE '%pg_stat_statements%'").splitlines():
        calls, rows, q = line.split("\t", 2)
        found = set()
        for m in WRITE.finditer(q):
            op, table = m.group(1).split()[0].upper(), m.group(2).lower()
            if table in ("set",):  # ON CONFLICT ... DO UPDATE SET
                continue
            found.add((op, table))
        for k in found:
            key = f"{k[0]} {k[1]}"
            c = out.setdefault(key, [0, 0])
            c[0] += int(calls)
            # A statement that writes several tables (a CTE) reports its rows once: give them to each.
            c[1] += int(rows)
    return out


class Observer:
    """A websocket that records the events it gets, tagged with the step running when they came."""

    def __init__(self, name, token, state):
        import websocket
        self.name, self.state, self.events = name, state, []
        self.ws = websocket.create_connection(BASE.replace("http", "ws") + "/api/v4/websocket",
                                              header=["Authorization: Bearer " + token])
        threading.Thread(target=self.read, daemon=True).start()

    def read(self):
        while True:
            try:
                m = json.loads(self.ws.recv())
            except Exception:  # noqa: BLE001
                return
            if "event" in m and m["event"] not in ("hello",):
                self.events.append((self.state["step"], m["event"]))


def main_run(impl, out_path):
    ctx = {}
    state = {"step": "connect"}
    admin, admin_id = login("admin", "Admin-pass-123")
    u1, u1_id = login("bench0101")
    u2, u2_id = login("bench0102")
    u3, u3_id = login("bench0103")
    spare, spare_id = login("bench0104")
    team = call(admin, "GET", "/teams/name/bench")[1]["id"]
    public = psql("SELECT Id FROM Channels WHERE Name = 'bench-public'").strip()
    bench00 = psql("SELECT Id FROM Channels WHERE Name = 'bench-00'").strip()
    observers = [Observer("u1", u1, state), Observer("u2", u2, state), Observer("admin", admin, state)]
    time.sleep(1)

    steps = []

    def step(name):
        def deco(fn):
            steps.append((name, fn))
            return fn
        return deco

    def keep(k, r):
        s, v, _ = r
        if isinstance(v, dict) and "id" in v and s < 300:
            ctx[k] = v["id"]
        return r

    # ---- session and the first load ----
    step("login")(lambda: call(None, "POST", "/users/login", {"login_id": "bench0105", "password": PASSWORD}))
    step("users_me")(lambda: call(u1, "GET", "/users/me"))
    step("my_teams")(lambda: call(u1, "GET", "/users/me/teams"))
    step("my_team_members")(lambda: call(u1, "GET", "/users/me/teams/members"))
    step("my_teams_unread")(lambda: call(u1, "GET", "/users/me/teams/unread"))
    step("my_channels")(lambda: call(u1, "GET", f"/users/me/teams/{team}/channels"))
    step("my_channel_members")(lambda: call(u1, "GET", f"/users/me/teams/{team}/channels/members"))
    step("preferences")(lambda: call(u1, "GET", "/users/me/preferences"))
    step("categories")(lambda: call(u1, "GET", f"/users/{u1_id}/teams/{team}/channels/categories"))
    step("status_ids")(lambda: call(u1, "POST", "/users/status/ids", [u1_id, u2_id, u3_id]))
    step("users_ids")(lambda: call(u1, "POST", "/users/ids", [u2_id, u3_id]))
    step("users_usernames")(lambda: call(u1, "POST", "/users/usernames", ["bench0102", "bench0103"]))
    step("get_user")(lambda: call(u1, "GET", f"/users/{u2_id}"))
    step("users_in_channel")(lambda: call(u1, "GET", f"/users?in_channel={bench00}&page=0&per_page=60"))
    step("users_in_team")(lambda: call(u1, "GET", f"/users?in_team={team}&page=0&per_page=60"))
    step("autocomplete")(lambda: call(u1, "GET", f"/users/autocomplete?in_team={team}&name=bench01"))
    step("channel_posts")(lambda: call(u1, "GET", f"/channels/{public}/posts?page=0&per_page=30"))
    step("posts_unread")(lambda: call(u1, "GET", f"/users/{u1_id}/channels/{public}/posts/unread?limit_before=30&limit_after=30"))
    step("view")(lambda: call(u1, "POST", "/channels/members/me/view", {"channel_id": public, "prev_channel_id": bench00}))
    step("channel_stats")(lambda: call(u1, "GET", f"/channels/{public}/stats"))
    step("channel_member_me")(lambda: call(u1, "GET", f"/channels/{public}/members/me"))
    step("threads")(lambda: call(u1, "GET", f"/users/{u1_id}/teams/{team}/threads?per_page=25"))

    # ---- preferences and status ----
    step("pref_save")(lambda: call(u1, "PUT", f"/users/{u1_id}/preferences",
                                   [{"user_id": u1_id, "category": "display_settings", "name": "use_military_time", "value": "true"}]))
    step("pref_delete")(lambda: call(u1, "POST", f"/users/{u1_id}/preferences/delete",
                                     [{"user_id": u1_id, "category": "display_settings", "name": "use_military_time", "value": "true"}]))
    step("status_away")(lambda: call(u1, "PUT", f"/users/{u1_id}/status", {"user_id": u1_id, "status": "away"}))
    step("status_dnd")(lambda: call(u1, "PUT", f"/users/{u1_id}/status",
                                    {"user_id": u1_id, "status": "dnd", "dnd_end_time": int(time.time()) + 3600}))
    step("status_online")(lambda: call(u1, "PUT", f"/users/{u1_id}/status", {"user_id": u1_id, "status": "online"}))
    step("custom_status")(lambda: call(u1, "PUT", f"/users/{u1_id}/status/custom", {"emoji": "calendar", "text": "meeting"}))
    step("custom_status_clear")(lambda: call(u1, "DELETE", f"/users/{u1_id}/status/custom"))

    # ---- channels ----
    step("channel_create")(lambda: keep("ch", call(u1, "POST", "/channels",
                                                    {"team_id": team, "name": f"sqldiff-{impl}", "display_name": "SQL diff", "type": "O"})))
    step("channel_patch")(lambda: call(u1, "PUT", f"/channels/{ctx['ch']}/patch", {"header": "a header", "purpose": "a purpose"}))
    step("channel_add_member")(lambda: call(u1, "POST", f"/channels/{ctx['ch']}/members", {"user_id": u2_id}))
    step("channel_add_members")(lambda: call(u1, "POST", f"/channels/{ctx['ch']}/members", {"user_ids": [u3_id, spare_id]}))
    step("channel_member_roles")(lambda: call(u1, "PUT", f"/channels/{ctx['ch']}/members/{u2_id}/schemeRoles",
                                              {"scheme_user": True, "scheme_admin": True}))
    step("channel_notify_props")(lambda: call(u2, "PUT", f"/channels/{ctx['ch']}/members/{u2_id}/notify_props",
                                              {"channel_id": ctx["ch"], "user_id": u2_id, "desktop": "all", "mark_unread": "all"}))
    step("channel_remove_member")(lambda: call(u1, "DELETE", f"/channels/{ctx['ch']}/members/{spare_id}"))
    step("channel_leave")(lambda: call(u3, "DELETE", f"/channels/{ctx['ch']}/members/{u3_id}"))
    step("channel_join")(lambda: call(u3, "POST", f"/channels/{ctx['ch']}/members", {"user_id": u3_id}))
    step("channel_privacy")(lambda: call(admin, "PUT", f"/channels/{ctx['ch']}/privacy", {"privacy": "P"}))
    step("channel_privacy_back")(lambda: call(admin, "PUT", f"/channels/{ctx['ch']}/privacy", {"privacy": "O"}))
    step("dm_create")(lambda: keep("dm", call(u1, "POST", "/channels/direct", [u1_id, u2_id])))
    step("gm_create")(lambda: keep("gm", call(u1, "POST", "/channels/group", [u1_id, u2_id, u3_id])))

    # ---- posts ----
    step("post")(lambda: keep("p1", call(u1, "POST", "/posts", {"channel_id": ctx["ch"], "message": "hello #tag"})))
    step("post_mention")(lambda: keep("p2", call(u1, "POST", "/posts", {"channel_id": ctx["ch"], "message": "@bench0102 look"})))
    step("post_channel_mention")(lambda: call(u1, "POST", "/posts", {"channel_id": ctx["ch"], "message": "@channel all of you"}))
    step("post_reply")(lambda: keep("r1", call(u2, "POST", "/posts", {"channel_id": ctx["ch"], "root_id": ctx["p1"], "message": "a reply"})))
    step("post_reply_mention")(lambda: call(u3, "POST", "/posts",
                                            {"channel_id": ctx["ch"], "root_id": ctx["p1"], "message": "@bench0101 in thread"}))
    step("post_dm")(lambda: call(u2, "POST", "/posts", {"channel_id": ctx["dm"], "message": "direct"}))
    step("post_gm")(lambda: call(u3, "POST", "/posts", {"channel_id": ctx["gm"], "message": "group"}))
    step("post_public")(lambda: call(u2, "POST", "/posts", {"channel_id": public, "message": "public hello"}))
    step("post_get")(lambda: call(u1, "GET", f"/posts/{ctx['p1']}"))
    step("post_thread")(lambda: call(u1, "GET", f"/posts/{ctx['p1']}/thread"))
    step("post_patch")(lambda: call(u1, "PUT", f"/posts/{ctx['p1']}/patch", {"message": "hello edited #tag"}))
    step("post_pin")(lambda: call(u1, "POST", f"/posts/{ctx['p1']}/pin"))
    step("pinned")(lambda: call(u1, "GET", f"/channels/{ctx['ch']}/pinned"))
    step("post_unpin")(lambda: call(u1, "POST", f"/posts/{ctx['p1']}/unpin"))
    step("reaction_add")(lambda: call(u2, "POST", "/reactions", {"user_id": u2_id, "post_id": ctx["p1"], "emoji_name": "smile"}))
    step("reactions")(lambda: call(u1, "GET", f"/posts/{ctx['p1']}/reactions"))
    step("reaction_remove")(lambda: call(u2, "DELETE", f"/users/{u2_id}/posts/{ctx['p1']}/reactions/smile"))
    step("flag")(lambda: call(u1, "PUT", f"/users/{u1_id}/preferences",
                              [{"user_id": u1_id, "category": "flagged_post", "name": ctx["p2"], "value": "true"}]))
    step("flagged")(lambda: call(u1, "GET", f"/users/{u1_id}/posts/flagged"))
    step("set_unread")(lambda: call(u2, "POST", f"/users/{u2_id}/posts/{ctx['p1']}/set_unread"))
    step("view_after_unread")(lambda: call(u2, "POST", "/channels/members/me/view", {"channel_id": ctx["ch"]}))
    step("search")(lambda: call(u1, "POST", f"/teams/{team}/posts/search", {"terms": "hello", "is_or_search": False, "page": 0, "per_page": 20}))
    step("hashtag_search")(lambda: call(u1, "POST", f"/teams/{team}/posts/search", {"terms": "#tag", "page": 0, "per_page": 20}))

    # ---- threads ----
    step("thread_follow")(lambda: call(u2, "PUT", f"/users/{u2_id}/teams/{team}/threads/{ctx['p1']}/following"))
    step("thread_read")(lambda: call(u1, "PUT", f"/users/{u1_id}/teams/{team}/threads/{ctx['p1']}/read/{int(time.time() * 1000)}"))
    step("thread_unread")(lambda: call(u1, "POST", f"/users/{u1_id}/teams/{team}/threads/{ctx['p1']}/set_unread/{ctx['r1']}"))
    step("thread_unfollow")(lambda: call(u2, "DELETE", f"/users/{u2_id}/teams/{team}/threads/{ctx['p1']}/following"))
    step("threads_read_all")(lambda: call(u1, "PUT", f"/users/{u1_id}/teams/{team}/threads/read"))

    # ---- files, drafts ----
    step("file_upload")(lambda: keep_file(call(u1, "POST", f"/files?channel_id={ctx['ch']}&filename=note.txt",
                                                raw=b"a small text file\n", ctype="text/plain")))

    def keep_file(r):
        s, v, _ = r
        if s < 300 and v and v.get("file_infos"):
            ctx["file"] = v["file_infos"][0]["id"]
        return r

    step("post_with_file")(lambda: call(u1, "POST", "/posts", {"channel_id": ctx["ch"], "message": "file", "file_ids": [ctx.get("file", "")]}))
    step("file_info")(lambda: call(u1, "GET", f"/files/{ctx.get('file', 'x' * 26)}/info"))
    step("draft_save")(lambda: call(u1, "POST", "/drafts", {"user_id": u1_id, "channel_id": ctx["ch"], "root_id": "", "message": "draft"}))
    step("drafts")(lambda: call(u1, "GET", f"/users/{u1_id}/teams/{team}/drafts"))
    step("draft_delete")(lambda: call(u1, "DELETE", f"/users/{u1_id}/channels/{ctx['ch']}/drafts"))

    # ---- sidebar ----
    step("category_create")(lambda: keep("cat", call(u1, "POST", f"/users/{u1_id}/teams/{team}/channels/categories",
                                                      {"user_id": u1_id, "team_id": team, "display_name": "Mine", "channel_ids": [ctx["ch"]]})))
    step("category_order")(lambda: call(u1, "GET", f"/users/{u1_id}/teams/{team}/channels/categories/order"))
    step("category_delete")(lambda: call(u1, "DELETE", f"/users/{u1_id}/teams/{team}/channels/categories/{ctx.get('cat', 'x' * 26)}"))

    # ---- commands and webhooks ----
    step("cmd_away")(lambda: call(u1, "POST", "/commands/execute", {"channel_id": ctx["ch"], "command": "/away"}))
    step("cmd_online")(lambda: call(u1, "POST", "/commands/execute", {"channel_id": ctx["ch"], "command": "/online"}))
    step("cmd_me")(lambda: call(u1, "POST", "/commands/execute", {"channel_id": ctx["ch"], "command": "/me waves"}))
    step("cmd_shrug")(lambda: call(u1, "POST", "/commands/execute", {"channel_id": ctx["ch"], "command": "/shrug ok"}))
    step("hook_create")(lambda: keep("hook", call(admin, "POST", "/hooks/incoming", {"channel_id": ctx["ch"], "display_name": "sqldiff"})))
    step("hook_post")(lambda: incoming(ctx.get("hook", "x" * 26)))

    def incoming(hook):
        req = urllib.request.Request(BASE + "/hooks/" + hook, data=json.dumps({"text": "from a hook"}).encode(),
                                     method="POST", headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                return r.status, None, r.headers
        except urllib.error.HTTPError as e:
            return e.code, None, e.headers

    step("hook_delete")(lambda: call(admin, "DELETE", f"/hooks/incoming/{ctx.get('hook', 'x' * 26)}"))

    # ---- deleting, archiving ----
    step("post_delete")(lambda: call(u1, "DELETE", f"/posts/{ctx['p2']}"))
    step("channel_archive")(lambda: call(u1, "DELETE", f"/channels/{ctx['ch']}"))
    step("channel_restore")(lambda: call(admin, "POST", f"/channels/{ctx['ch']}/restore"))

    # ---- users and teams ----
    step("user_patch")(lambda: call(u1, "PUT", f"/users/{u1_id}/patch", {"nickname": "sqldiff", "position": "tester"}))
    step("user_notify_props")(lambda: call(u1, "PUT", f"/users/{u1_id}/patch", {"notify_props": {"email": "false", "push": "mention", "desktop": "all"}}))
    step("user_roles")(lambda: call(admin, "PUT", f"/users/{spare_id}/roles", {"roles": "system_user system_user_manager"}))
    step("user_roles_back")(lambda: call(admin, "PUT", f"/users/{spare_id}/roles", {"roles": "system_user"}))
    step("user_deactivate")(lambda: call(admin, "PUT", f"/users/{spare_id}/active", {"active": False}))
    step("user_activate")(lambda: call(admin, "PUT", f"/users/{spare_id}/active", {"active": True}))
    step("team_remove_member")(lambda: call(admin, "DELETE", f"/teams/{team}/members/{spare_id}"))
    step("team_add_member")(lambda: call(admin, "POST", f"/teams/{team}/members", {"team_id": team, "user_id": spare_id}))
    step("team_member_roles")(lambda: call(admin, "PUT", f"/teams/{team}/members/{spare_id}/schemeRoles",
                                           {"scheme_user": True, "scheme_admin": False}))
    step("team_stats")(lambda: call(u1, "GET", f"/teams/{team}/stats"))
    step("password")(lambda: call(u3, "PUT", f"/users/{u3_id}/password", {"current_password": PASSWORD, "new_password": PASSWORD}))
    step("sessions")(lambda: call(u1, "GET", f"/users/{u1_id}/sessions"))
    step("logout")(lambda: call(u3, "POST", "/users/logout"))

    results = []
    for name, fn in steps:
        state["step"] = name
        psql("SELECT pg_stat_statements_reset()")
        try:
            s, v, _ = fn()
        except Exception as e:  # noqa: BLE001
            s, v = "error", repr(e)[:200]
        time.sleep(1.2)
        w = writes()
        results.append({"step": name, "status": s, "keys": keys(v) if s != "error" else v, "writes": w})
        print(f"{impl} {name}: {s} {', '.join(f'{k} {c[1]}' for k, c in sorted(w.items()))}", flush=True)
    time.sleep(1)
    events = {o.name: o.events for o in observers}
    json.dump({"steps": results, "events": events}, open(out_path, "w"), indent=1)


def main_compare(go_path, rust_path):
    g, r = json.load(open(go_path)), json.load(open(rust_path))
    gs = {s["step"]: s for s in g["steps"]}
    ev = {}
    for which, d in (("go", g), ("rust", r)):
        for obs, lst in d["events"].items():
            for step, e in lst:
                ev.setdefault(step, {}).setdefault(obs, {}).setdefault(e, {"go": 0, "rust": 0})[which] += 1
    n_diff = 0
    for s in r["steps"]:
        name, o = s["step"], gs.get(s["step"])
        lines = []
        if o is None:
            continue
        if o["status"] != s["status"] or o["keys"] != s["keys"]:
            lines.append(f"  answer   go {o['status']} {o['keys']}\n           rust {s['status']} {s['keys']}")
        for k in sorted(set(o["writes"]) | set(s["writes"])):
            if k.split()[1] in BACKGROUND:
                continue
            a, b = o["writes"].get(k, [0, 0]), s["writes"].get(k, [0, 0])
            if a[1] != b[1] or (a[0] == 0) != (b[0] == 0):
                lines.append(f"  {k:34} go {a[0]} stmts {a[1]} rows | rust {b[0]} stmts {b[1]} rows")
        for obs, es in sorted(ev.get(name, {}).items()):
            for e, c in sorted(es.items()):
                if c["go"] != c["rust"]:
                    lines.append(f"  event {obs:5} {e:32} go {c['go']} rust {c['rust']}")
        if lines:
            n_diff += 1
            print(name)
            print("\n".join(lines))
    print(f"\n{n_diff} of {len(r['steps'])} steps differ")


if __name__ == "__main__":
    if sys.argv[1] == "run":
        main_run(sys.argv[2], sys.argv[3])
    else:
        main_compare(sys.argv[2], sys.argv[3])
