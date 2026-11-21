#!/usr/bin/env python3
"""What each server writes to the database, step by step, for the same client scenario.

One server at a time on :8065, started from the same data each time (dev/bench/sqldiff.sh restores a snapshot), with
pg_stat_statements loaded. Before every step the statistics are reset; after it (and a pause for the work done after
the answer) the writes are read back as (operation, table, rows). SQL texts differ between Go and Rust, so the writes
are compared by what they touch, not by their text. Also kept per step: the status, the answer's top-level keys and
the websocket events three observers received; and, for the steps that opt in (s.body(call(...))), the answer
itself, normalized (see normalize), compared by value: the first differing paths are printed.

    sqldiff.py run <impl> <out.json>         against SQLDIFF_URL (default :8065); SQLDIFF_SCENARIO=core,... picks
                                             the scenario modules (tools/diff/scen_<name>.py, default core)
    sqldiff.py compare <go.json> <rust.json> the differences, step by step

Stdlib and websocket-client only.
"""

import importlib
import json
import os
import types
import re
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request

BASE = os.environ.get("SQLDIFF_URL", "http://localhost:8065")
PG = os.environ.get("SQLDIFF_PG", "bench-db")
# The database this server runs on: two servers can run side by side on two copies, and pg_stat_statements is read
# and reset per database.
DB = os.environ.get("SQLDIFF_DB", "mattermost")
PASSWORD = "Bench-pass-123"
# Writes that come from timers rather than from the step (Go's job scheduler, cluster and status flushes).
BACKGROUND = {"jobs", "systems", "clusterdiscovery", "pluginkeyvaluestore", "postreminders"}


def psql(sql):
    return subprocess.check_output(
        ["docker", "exec", PG, "psql", "-U", "mmuser", "-d", DB, "-AtF\t", "-c", sql]).decode()


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
    for line in psql("SELECT calls, rows, regexp_replace(query, '\\s+', ' ', 'g') FROM pg_stat_statements "
                     "WHERE dbid = (SELECT oid FROM pg_database WHERE datname = current_database()) "
                     "AND query NOT ILIKE '%pg_stat_statements%'").splitlines():
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


def reset():
    psql("SELECT pg_stat_statements_reset(0, (SELECT oid FROM pg_database WHERE datname = current_database()), 0)")


def settle():
    """The step's writes once they stop changing: the work a server does after its answer (notifications, status
    flushes) lands within a few hundred milliseconds; at most 3 s."""
    time.sleep(0.3)
    last, t0 = writes(), time.time()
    while time.time() - t0 < 3:
        time.sleep(0.25)
        cur = writes()
        if cur == last:
            return cur
        last = cur
    return last


IDRE = re.compile(r"(?<![a-z0-9])[a-z0-9]{26}(?![a-z0-9])")
TS_KEY = re.compile(r"(^|_)at$|^last_|_time$")


def normalize(v, unordered=()):
    """An answer as it can be compared between two servers: 26-character ids become <id1>, <id2>... in order of
    first appearance (in keys and inside strings too), millisecond timestamps (13 digits, or a nonzero number in a
    *_at / last_* / *_time field) become <ts>. Object members are visited ids already seen first (a post list's
    posts after its order), then by key: the order of a JSON object means nothing. Arrays at the paths in
    unordered ('' the top, 'a.*.b' through arrays) are sorted first, by their items with ids and timestamps blanked,
    then by the raw items (ids of the snapshot's rows are the same on both sides)."""
    ids = {}

    def sid(m):
        return ids.setdefault(m.group(0), f"<id{len(ids) + 1}>")

    def ts(x, key):
        return isinstance(x, int) and not isinstance(x, bool) and (
            10**12 <= abs(x) < 10**13 or (x and key and TS_KEY.search(key)))

    def blank(x, key=None):
        if isinstance(x, list):
            return [blank(i) for i in x]
        if isinstance(x, dict):
            return {IDRE.sub("~", k): blank(x[k], k) for k in x}
        if isinstance(x, str):
            return IDRE.sub("~", x)
        return 0 if ts(x, key) else x

    def order(k):
        return (0, int(ids[k][3:-1]), "") if k in ids else (1, 0, k)

    def walk(x, path, key):
        if isinstance(x, list):
            if path in unordered:
                x = sorted(x, key=lambda i: (json.dumps(blank(i), sort_keys=True), json.dumps(i, sort_keys=True)))
            return [walk(i, f"{path}.*".lstrip("."), None) for i in x]
        if isinstance(x, dict):
            return {IDRE.sub(sid, k): walk(x[k], f"{path}.{k}".lstrip("."), k) for k in sorted(x, key=order)}
        if isinstance(x, str):
            return IDRE.sub(sid, x)
        return "<ts>" if ts(x, key) else x
    return walk(v, "", None)


def body_diff(a, b, path="", out=None):
    """Paths where two normalized answers differ, as 'path: go -> rust'."""
    out = [] if out is None else out
    if isinstance(a, dict) and isinstance(b, dict):
        for k in list(a) + [k for k in b if k not in a]:
            if k not in a or k not in b:
                out.append(f"{path}.{k}: {'missing' if k not in a else json.dumps(a[k])[:80]} -> "
                           f"{'missing' if k not in b else json.dumps(b[k])[:80]}")
            else:
                body_diff(a[k], b[k], f"{path}.{k}", out)
    elif isinstance(a, list) and isinstance(b, list):
        if len(a) != len(b):
            out.append(f"{path}: {len(a)} items -> {len(b)} items")
        for i, (x, y) in enumerate(zip(a, b)):
            body_diff(x, y, f"{path}[{i}]", out)
    elif a != b or type(a) is not type(b):
        out.append(f"{path}: {json.dumps(a)[:80]} -> {json.dumps(b)[:80]}")
    return out


class Scenario(types.SimpleNamespace):
    """What a scenario module's register(s) gets: the step decorator, call, keep (remember an id), ctx (ids made
    so far), the logged-in users (u1..u3, spare: tokens and ids), admin, the bench team, bench-public and bench-00,
    the base URL, psql and login."""


class Observer:
    """A websocket that records the events it gets, tagged with the step running when they came."""

    def __init__(self, name, token, state):
        import websocket
        self.name, self.state, self.events = name, state, []
        self.ws = websocket.create_connection(BASE.replace("http", "ws") + "/api/v4/websocket",
                                              header=["Authorization: Bearer " + token], suppress_origin=True)
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

    def body(r, unordered=()):
        """Opt-in: record this step's answer body, normalized, to be compared by value."""
        state["body"] = normalize(r[1], unordered)
        return r

    def keep(k, r):
        s, v, _ = r
        if isinstance(v, dict) and "id" in v and s < 300:
            ctx[k] = v["id"]
        return r

    s = Scenario(impl=impl, step=step, call=call, keep=keep, ctx=ctx, u1=u1, u1_id=u1_id, u2=u2, u2_id=u2_id,
                 u3=u3, u3_id=u3_id, admin=admin, admin_id=admin_id, spare=spare, spare_id=spare_id, team=team,
                 public=public, bench00=bench00, base=BASE, password=PASSWORD, psql=psql, login=login,
                 observers=observers, state=state, body=body)
    for name in os.environ.get("SQLDIFF_SCENARIO", "core").split(","):
        importlib.import_module(f"scen_{name}").register(s)

    results = []
    # SQLDIFF_ONLY=prefix,prefix records only the matching steps; the others still run (later steps need what they
    # make), just without the pause and the statistics. SQLDIFF_UPTO=step stops after that step.
    only = [p for p in os.environ.get("SQLDIFF_ONLY", "").split(",") if p]
    upto = os.environ.get("SQLDIFF_UPTO", "")
    for name, fn in steps:
        state["step"] = name
        if only and not any(name.startswith(p) for p in only):
            try:
                fn()
            except Exception:  # noqa: BLE001
                pass
            if name == upto:
                break
            continue
        reset()
        state.pop("body", None)
        try:
            s, v, _ = fn()
        except Exception as e:  # noqa: BLE001
            s, v = "error", repr(e)[:200]
        w = settle()
        results.append({"step": name, "status": s, "keys": keys(v) if s != "error" else v, "writes": w})
        print(f"{impl} {name}: {s} {', '.join(f'{k} {c[1]}' for k, c in sorted(w.items()))}", flush=True)
        if "body" in state:
            results[-1]["body"] = state.pop("body")
        if name == upto:
            break
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
            # Rows, not statements: a statement one server runs on an empty set (Go's mention update with no
            # mentions, a sidebar delete of nothing) changes nothing the other server missed.
            if a[1] != b[1]:
                lines.append(f"  {k:34} go {a[0]} stmts {a[1]} rows | rust {b[0]} stmts {b[1]} rows")
        if "body" in o and "body" in s:
            d, n = body_diff(o["body"], s["body"]), int(os.environ.get("SQLDIFF_BODY_LINES", "12"))
            lines += [f"  body     {x}" for x in d[:n]]
            if len(d) > n:
                lines.append(f"  body     ... {len(d)} differences")
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
