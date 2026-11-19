#!/usr/bin/env python3
"""Same requests to the Go server and the Rust server, answers compared.

EBAChat's calls (../ebac-messenger/src/api.rs) in the order the client makes them, as one user. Reads are compared
as they are: status, keys and key order, types and values. Writes go to both servers and are compared by shape
(their ids and times differ). Stdlib only.

    tools/diff/diff.py [--go http://localhost:8065] [--rust http://localhost:8066] [--user admin --password ...]
"""

import argparse
import json
import re
import sys
import urllib.error
import urllib.request

ID = re.compile(r"^[a-z0-9]{26}$")


def call(base, token, method, path, body=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(base + "/api/v4" + path, data=data, method=method)
    if token:
        req.add_header("Authorization", "Bearer " + token)
    if data is not None:
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            raw, status, headers = r.read(), r.status, r.headers
    except urllib.error.HTTPError as e:
        raw, status, headers = e.read(), e.code, e.headers
    ctype = headers.get("Content-Type", "")
    try:
        val = json.loads(raw, object_pairs_hook=lambda kv: kv) if "json" in ctype or raw[:1] in b"[{" else raw
    except ValueError:
        val = raw
    return status, val, headers


def login(base, user, password):
    s, _, h = call(base, None, "POST", "/users/login", {"login_id": user, "password": password})
    if s != 200:
        sys.exit(f"login to {base}: {s}")
    return h["Token"]


# Values that differ between two servers by nature, or between two writes.
VOLATILE = {"request_id", "server_time", "last_activity_at", "BuildDate", "BuildHash", "BuildHashEnterprise",
            "BuildNumber", "Version", "DiagnosticId", "TelemetryId", "ServiceEnvironment"}


def norm_pairs(v):
    """Pairs (key order kept) → nested lists we can compare."""
    if isinstance(v, list) and v and all(isinstance(x, tuple) and len(x) == 2 for x in v):
        return ("obj", [(k, norm_pairs(x)) for k, x in v])
    if isinstance(v, list):
        return ("arr", [norm_pairs(x) for x in v])
    return v


def diff(a, b, path, shape_only, out):
    if isinstance(a, tuple) and isinstance(b, tuple) and a[0] == b[0] == "obj":
        ka, kb = [k for k, _ in a[1]], [k for k, _ in b[1]]
        for k in ka:
            if k not in kb:
                out.append(f"{path}.{k}: only in Go")
        for k in kb:
            if k not in ka:
                out.append(f"{path}.{k}: only in Rust")
        common = [k for k in ka if k in kb]
        if [k for k in kb if k in ka] != common and not path.endswith("posts") and not path.endswith("file_infos"):
            out.append(f"{path}: key order Go {common} Rust {[k for k in kb if k in ka]}")
        da, db = dict(a[1]), dict(b[1])
        for k in common:
            if k in VOLATILE:
                continue
            # maps keyed by ids: compare by key
            diff(da[k], db[k], f"{path}.{k}", shape_only, out)
        return
    if isinstance(a, tuple) and isinstance(b, tuple) and a[0] == b[0] == "arr":
        if not shape_only and len(a[1]) != len(b[1]):
            out.append(f"{path}: {len(a[1])} items in Go, {len(b[1])} in Rust")
        for i, (x, y) in enumerate(zip(a[1], b[1])):
            diff(x, y, f"{path}[{i}]", shape_only, out)
        return
    ta, tb = type(a).__name__, type(b).__name__
    if ta != tb and not ({ta, tb} <= {"int", "float"}):
        out.append(f"{path}: {ta} {short(a)} in Go, {tb} {short(b)} in Rust")
        return
    if shape_only:
        return
    if a != b:
        out.append(f"{path}: {short(a)} in Go, {short(b)} in Rust")


def short(v):
    s = json.dumps(v, default=str) if not isinstance(v, bytes) else f"<{len(v)} bytes>"
    return s if len(s) < 80 else s[:77] + "..."


class Run:
    def __init__(self, go, rust, tg, tr):
        self.go, self.rust, self.tg, self.tr = go, rust, tg, tr
        self.failed = 0
        self.n = 0

    def check(self, method, path, body=None, shape_only=False, ignore=()):
        self.n += 1
        sa, va, ha = call(self.go, self.tg, method, path, body)
        sb, vb, hb = call(self.rust, self.tr, method, path, body)
        out = []
        if sa != sb:
            out.append(f"status {sa} in Go, {sb} in Rust" + (f": {short(norm_pairs(vb))}" if sb >= 400 else ""))
        if isinstance(va, (bytes, bytearray)) or isinstance(vb, (bytes, bytearray)):
            if (ha.get("Content-Type"), ) != (hb.get("Content-Type"), ):
                out.append(f"content-type {ha.get('Content-Type')} in Go, {hb.get('Content-Type')} in Rust")
        else:
            diff(norm_pairs(va), norm_pairs(vb), "", shape_only, out)
        out = [o for o in out if not any(i in o for i in ignore)]
        label = f"{method} {path}"
        if out:
            self.failed += 1
            print(f"DIFF {label}")
            for o in out[:25]:
                print("   ", o)
            if len(out) > 25:
                print(f"    ... {len(out) - 25} more")
        else:
            print(f"ok   {label}")
        return norm_pairs(va), norm_pairs(vb)


def get(v, *keys):
    for k in keys:
        if isinstance(v, tuple) and v[0] == "obj":
            v = dict(v[1]).get(k)
        elif isinstance(v, tuple) and v[0] == "arr":
            v = v[1][k] if k < len(v[1]) else None
        else:
            return None
    return v


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--go", default="http://localhost:8065")
    ap.add_argument("--rust", default="http://localhost:8066")
    ap.add_argument("--user", default="bench0001")
    ap.add_argument("--password", default="Bench-pass-123")
    ap.add_argument("--writes", action="store_true", help="also posts, reactions, drafts (on both servers)")
    a = ap.parse_args()

    tg = login(a.go, a.user, a.password)
    tr = login(a.rust, a.user, a.password) if a.rust != a.go else tg
    r = Run(a.go, a.rust, tg, tr)

    me, _ = r.check("GET", "/users/me")
    uid = get(me, "id")
    r.check("GET", "/config/client?format=old")
    r.check("GET", "/license/client?format=old")
    teams, _ = r.check("GET", "/users/me/teams")
    team = get(teams, 0, "id")
    r.check("GET", "/users/me/teams/unread?include_collapsed_threads=true")
    r.check("GET", f"/teams/name/{get(teams, 0, 'name')}")
    chans, _ = r.check("GET", f"/users/me/teams/{team}/channels")
    r.check("GET", f"/users/me/teams/{team}/channels/members")
    r.check("GET", f"/users/me/teams/{team}/channels/categories")
    r.check("GET", "/users/me/preferences")
    ch = get(chans, 0, "id")
    r.check("GET", f"/channels/{ch}")
    r.check("GET", f"/channels/{ch}/stats")
    r.check("GET", f"/channels/{ch}/stats?exclude_files_count=false")
    r.check("GET", f"/channels/{ch}/members/me")
    r.check("GET", f"/channels/{ch}/timezones")
    r.check("GET", f"/channels/{ch}/pinned")
    posts, _ = r.check("GET", f"/channels/{ch}/posts?per_page=60&collapsedThreads=true&collapsedThreadsExtended=true")
    order = get(posts, "order")
    pid = order[1][0] if order and order[1] else None
    r.check("GET", f"/channels/{ch}/posts?since=0&collapsedThreads=true&collapsedThreadsExtended=true")
    r.check("GET", f"/channels/{ch}/posts?since=4102444800000")
    if pid:
        r.check("GET", f"/posts/{pid}")
        r.check("GET", f"/posts/{pid}/thread")
        r.check("GET", f"/channels/{ch}/posts?per_page=60&collapsedThreads=true&collapsedThreadsExtended=true&before={pid}")
        r.check("GET", f"/channels/{ch}/posts?per_page=60&collapsedThreads=true&collapsedThreadsExtended=true&after={pid}")
    r.check("POST", "/channels/members/me/view", {"channel_id": ch})
    r.check("GET", f"/users/me/teams/{team}/threads?extended=true&per_page=25")
    r.check("GET", f"/users/me/teams/{team}/threads?extended=true&per_page=25&unread=true")
    r.check("GET", f"/users/{uid}/teams/{team}/drafts")
    users, _ = r.check("POST", "/users/ids", [uid])
    r.check("POST", "/users/status/ids", [uid])
    r.check("POST", "/users/usernames", [a.user])
    r.check("GET", f"/users/{uid}")
    r.check("GET", f"/users/username/{a.user}")
    r.check("GET", f"/users/autocomplete?in_channel={ch}&in_team={team}&name=bench&limit=25")
    r.check("GET", f"/teams/{team}/channels/autocomplete?name=bench")
    r.check("GET", f"/users?page=0&per_page=60&active=true&in_team={team}")
    r.check("GET", f"/users?in_channel={ch}&page=0&per_page=60&sort=admin&active=true")
    r.check("POST", "/users/search", {"term": "bench00", "team_id": team, "allow_inactive": False, "limit": 20})
    r.check("GET", "/users/stats")
    r.check("GET", f"/teams/{team}/members/me")
    r.check("POST", f"/teams/{team}/members/ids", [uid])
    r.check("POST", f"/channels/{ch}/members/ids", [uid])
    r.check("POST", "/roles/names", ["system_user", "team_user", "channel_user"])
    r.check("GET", "/emoji?page=0&per_page=200&sort=name")
    r.check("POST", "/emoji/search", {"term": "a"})
    r.check("GET", f"/teams/{team}/channels?page=0&per_page=200")
    r.check("GET", f"/teams/{team}/channels/deleted?page=0&per_page=200")
    r.check("GET", f"/users/me/teams/{team}/channels?include_deleted=true")
    r.check("POST", "/channels/stats/member_count", [ch])
    r.check("POST", "/channels/search?system_console=false", {"term": "bench", "team_ids": [team], "include_deleted": True})
    r.check("GET", f"/teams/{team}/channels/name/{get(chans, 0, 'name')}?include_deleted=true")
    r.check("GET", f"/channels/{ch}/common_teams")
    r.check("GET", f"/users/{uid}/image", ignore=("content-type",))
    r.check("GET", "/users/me/posts/flagged?page=0&per_page=60".replace("/users/me", f"/users/{uid}"))
    r.check("POST", f"/teams/{team}/posts/search", {"terms": "hello", "is_or_search": False, "time_zone_offset": 0,
                                                   "include_deleted_channels": True, "page": 0, "per_page": 60})
    r.check("POST", f"/teams/{team}/files/search", {"terms": "a", "is_or_search": False, "time_zone_offset": 0,
                                                   "include_deleted_channels": True, "page": 0, "per_page": 60})
    if a.writes:
        pg, pr = r.check("POST", "/posts", {"channel_id": ch, "root_id": "", "message": "diff check", "file_ids": []},
                         shape_only=True)
        r.check("POST", "/users/me/typing", {"channel_id": ch, "parent_id": ""})
        r.check("POST", "/drafts", {"user_id": uid, "channel_id": ch, "root_id": "", "message": "draft", "file_ids": []},
                shape_only=True)
        r.check("DELETE", f"/users/{uid}/channels/{ch}/drafts", shape_only=True)
    print(f"\n{r.n - r.failed}/{r.n} identical")
    sys.exit(1 if r.failed else 0)


if __name__ == "__main__":
    main()
