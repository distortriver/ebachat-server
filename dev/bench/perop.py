"""Every SQL statement each server runs for one request of each kind: pg_stat_statements reset, N requests of one
kind from N different users, a pause for work done after the answer, then the statements per request.

    perop.py <impl> <out.json> [N]     (the server already running on :8065, pg_stat_statements loaded)
"""
import json
import subprocess
import sys
import time
import urllib.request

import websocket

IMPL, OUT = sys.argv[1], sys.argv[2]
N = int(sys.argv[3]) if len(sys.argv) > 3 else 40
BASE = "http://localhost:8065/api/v4"
RUN = "/home/ubuntu/ebachat-server/dev/bench/run"


def psql(sql):
    return subprocess.check_output(
        ["docker", "exec", "bench-db", "psql", "-U", "mmuser", "-d", "mattermost", "-AtF\t", "-c", sql]).decode()


def call(method, path, tok, body=None):
    data = json.dumps(body).encode() if body is not None else None
    rq = urllib.request.Request(BASE + path, data=data, method=method,
                                headers={"Authorization": "Bearer " + tok, "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(rq) as r:
            r.read()
            return r.status
    except urllib.error.HTTPError as e:
        return e.code


tokens = [l.split("\t") for l in open(RUN + "/tokens.tsv").read().split("\n") if l]
users = [u for u, _ in tokens]
public = psql("SELECT Id FROM Channels WHERE Name = 'bench-public'").strip()
member = {}
for row in psql("""SELECT cm.UserId, cm.ChannelId FROM ChannelMembers cm JOIN Channels c ON c.Id = cm.ChannelId
                   WHERE c.Type = 'O' AND c.Name LIKE 'bench-__' ORDER BY 1, 2""").splitlines():
    u, c = row.split("\t")
    member.setdefault(u, []).append(c)
# Users 100.. so that no earlier probe warmed them; each has at least two bench channels.
sample = [(u, t) for u, t in tokens[100:] if len(member.get(u, [])) >= 2][:N]


def ws_connect(u, t, i):
    ws = websocket.create_connection("ws://localhost:8065/api/v4/websocket", header=["Authorization: Bearer " + t])
    ws.recv()
    ws.close()
    return 101


OPS = {
    "users_me": lambda u, t, i: call("GET", "/users/me", t),
    "status_ids": lambda u, t, i: call("POST", "/users/status/ids", t, users[i:i + 50]),
    "users_ids": lambda u, t, i: call("POST", "/users/ids", t, users[i + 1:i + 21]),
    "view": lambda u, t, i: call("POST", "/channels/members/me/view", t,
                                 {"channel_id": member[u][0], "prev_channel_id": member[u][1], "collapsed_threads_supported": True}),
    "posts_since": lambda u, t, i: call("GET", f"/channels/{member[u][0]}/posts?since={int(time.time() * 1000) - 60000}", t),
    "create_post": lambda u, t, i: call("POST", "/posts", t, {"channel_id": member[u][0], "message": f"perop {IMPL} {i}"}),
    "create_post_public": lambda u, t, i: call("POST", "/posts", t, {"channel_id": public, "message": f"perop public {IMPL} {i}"}),
    "ws_connect": ws_connect,
}

result = {}
for name, fn in OPS.items():
    time.sleep(2)
    psql("SELECT pg_stat_statements_reset()")
    codes = [fn(u, t, i) for i, (u, t) in enumerate(sample)]
    time.sleep(3)  # work after the answer: notifications, status writes, websocket fan-out
    rows = psql("""SELECT calls, rows, round(total_exec_time::numeric, 2), regexp_replace(query, '\\s+', ' ', 'g')
                   FROM pg_stat_statements WHERE query NOT ILIKE '%pg_stat_statements%' ORDER BY calls DESC, query""")
    stmts = []
    for r in rows.splitlines():
        c, n, ms, q = r.split("\t", 3)
        stmts.append({"per_req": int(c) / len(sample), "rows_per_req": int(n) / len(sample), "ms": float(ms), "query": q})
    result[name] = {"codes": sorted(set(codes)), "n": len(sample), "statements": stmts}
    print(f"{IMPL} {name}: codes {sorted(set(codes))}, {sum(s['per_req'] for s in stmts):.1f} statements/request", flush=True)
json.dump(result, open(OUT, "w"), indent=1)
