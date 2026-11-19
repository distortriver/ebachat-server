#!/usr/bin/env python3
"""Send the same request to Go (:8065) and Rust (RUST, default :8073) and diff the answers.

    dev/cmp.py [-t token|admin|bench] METHOD PATH [JSON]

Ids of errors' request_id are ignored; key order is compared too (Go's field order matters to no client, but
we keep it).
"""
import json
import os
import sys
import urllib.request

GO = "http://127.0.0.1:8065/api/v4"
RUST = os.environ.get("RUST", "http://127.0.0.1:8073/api/v4")
CACHE = os.path.join(os.path.dirname(__file__), "run", "cmp-tokens.json")


def login(user, pw):
    req = urllib.request.Request(GO + "/users/login", data=json.dumps({"login_id": user, "password": pw}).encode(),
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req) as r:
        return r.headers["Token"]


def token(who):
    try:
        cache = json.load(open(CACHE))
    except Exception:
        cache = {}
    if who in cache:
        return cache[who]
    t = login("admin", "Admin-pass-123") if who == "admin" else login("bench0001", "Bench-pass-123")
    cache[who] = t
    json.dump(cache, open(CACHE, "w"))
    return t


def call(base, method, path, body, tok):
    data = body.encode() if body is not None else None
    req = urllib.request.Request(base + path, data=data, method=method, headers={"Authorization": "Bearer " + tok})
    try:
        with urllib.request.urlopen(req) as r:
            return r.status, r.read(), dict(r.headers)
    except urllib.error.HTTPError as e:
        return e.code, e.read(), dict(e.headers)


def norm(b):
    try:
        v = json.loads(b)
    except Exception:
        return b[:200]

    def strip(x):
        if isinstance(x, dict):
            return {k: strip(v) for k, v in x.items() if k not in ("request_id",)}
        if isinstance(x, list):
            return [strip(i) for i in x]
        return x
    return strip(v)


def main():
    args = sys.argv[1:]
    who = "admin"
    if args[0] == "-t":
        who, args = args[1], args[2:]
    method, path = args[0], args[1]
    body = args[2] if len(args) > 2 else None
    tok = token(who) if who in ("admin", "bench") else who
    g = call(GO, method, path, body, tok)
    r = call(RUST, method, path, body, tok)
    gs, rs = norm(g[1]), norm(r[1])
    same = g[0] == r[0] and gs == rs
    print(f"{'SAME' if same else 'DIFF'} {method} {path}: go {g[0]} rust {r[0]}")
    if not same or os.environ.get("V"):
        print("go:  ", json.dumps(gs)[:1500])
        print("rust:", json.dumps(rs)[:1500])
    elif isinstance(gs, dict) and list(gs) != list(rs):
        print("key order differs:", list(gs), list(rs))


main()
