#!/usr/bin/env python3
"""Every route of docs/map/routes.tsv sent to Go and to Rust, as admin, with placeholder ids: status, error id and
top-level keys compared. A coarse net for routes no client scenario covers (T3, the unlicensed answers).

    tools/diff/routes.py [--area teams,users] [--tier T3,skip] [--go URL] [--rust URL]
"""

import argparse
import csv
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(__file__))
import diff  # noqa: E402

ID = "abcdefghijklmnopqrstuvwxyz"


def summary(status, v):
    v = diff.norm_pairs(v)
    if isinstance(v, tuple) and v[0] == "obj":
        d = dict(v[1])
        if status >= 400:
            return status, d.get("id")
        return status, tuple(sorted(k for k, _ in v[1]))
    if isinstance(v, tuple) and v[0] == "arr":
        return status, "array"
    return status, type(v).__name__


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--go", default="http://localhost:8065")
    ap.add_argument("--rust", default="http://localhost:8066")
    ap.add_argument("--area", default="")
    ap.add_argument("--tier", default="")
    a = ap.parse_args()
    tg = diff.login(a.go, "admin", "Admin-pass-123")
    tr = diff.login(a.rust, "admin", "Admin-pass-123")
    areas = set(filter(None, a.area.split(",")))
    tiers = set(filter(None, a.tier.split(",")))
    root = os.path.join(os.path.dirname(__file__), "..", "..", "docs", "map", "routes.tsv")
    same = diff_n = 0
    for r in csv.DictReader(open(root), delimiter="\t"):
        if areas and r["area"] not in areas or tiers and r["tier"] not in tiers:
            continue
        if r["method"] == "DELETE" and "/users/{user_id}" == r["path"][len("/api/v4"):]:
            continue  # never deactivate users from a sweep
        path = re.sub(r"\{[^}]+\}", ID, r["path"])[len("/api/v4"):]
        body = {} if r["method"] in ("POST", "PUT", "PATCH") else None
        g = summary(*diff.call(a.go, tg, r["method"], path, body)[:2])
        s = summary(*diff.call(a.rust, tr, r["method"], path, body)[:2])
        if g == s:
            same += 1
        else:
            diff_n += 1
            print(f"DIFF {r['tier']:4} {r['method']:6} {r['path']}\n     Go   {g}\n     Rust {s}")
    print(f"\n{same} same, {diff_n} different")


if __name__ == "__main__":
    main()
