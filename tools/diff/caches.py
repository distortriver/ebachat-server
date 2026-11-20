#!/usr/bin/env python3
"""Do the caches work, and do writes make them fresh?

For every cache in front of the database (docs/CACHES.md), the read it serves is done twice, then the write that
must invalidate it, then the read again:

- The answers must show the write. That is checked on both servers: Go and Rust must agree.
- On the Rust server, the cache's counters (`GET /api/v4/ebachat/caches`, admins) must show a hit for the repeated
  read and an invalidation for the write.

Run each server alone on the database (start one, run, stop it, start the other), as they take turns in production:

    tools/diff/caches.py http://localhost:8065      # Go: the answers only
    tools/diff/caches.py http://localhost:8066      # Rust: the answers and the counters
"""

import json
import sys
import time

sys.path.insert(0, __file__.rsplit("/", 1)[0])
import diff  # noqa: E402

PASSWORD = "Bench-pass-123"


def j(v):
    """The harness keeps key order as pairs; plain dicts and lists here."""
    if isinstance(v, list) and v and all(isinstance(x, tuple) and len(x) == 2 for x in v):
        return {k: j(x) for k, x in v}
    if isinstance(v, list):
        return [j(x) for x in v]
    return v


class S:
    def __init__(self, base):
        self.base = base
        # The Rust server is the one with the counters route (any port).
        self.rust = diff.call(base, None, "GET", "/ebachat/caches")[0] != 404
        self.admin = diff.login(base, "admin", "Admin-pass-123")
        self.fail = 0
        self.n = 0

    def call(self, tok, method, path, body=None, ok=(200, 201)):
        status, v, _ = diff.call(self.base, tok, method, path, body)
        if ok and status not in ok:
            raise RuntimeError(f"{method} {path}: {status} {v!r:.300}")
        return status, j(v)

    def stats(self):
        if not self.rust:
            return None
        return self.call(self.admin, "GET", "/ebachat/caches")[1]

    def check(self, name, cond, detail=""):
        self.n += 1
        if cond:
            print(f"ok    {name}")
        else:
            self.fail += 1
            print(f"FAIL  {name} {detail}")

    def counted(self, cache, before, field, name):
        """On Rust: the counter `field` of `cache` went up since `before`."""
        if not self.rust:
            return
        after = self.stats()
        self.check(f"{name}: {cache}.{field} counted", after[cache][field] > before[cache][field],
                   f"{before[cache][field]} → {after[cache][field]}")


def hook_post(base, hook_id, text, channel=""):
    import urllib.error
    import urllib.request
    req = urllib.request.Request(f"{base}/hooks/{hook_id}", data=json.dumps({"text": text, "channel": channel}).encode(), method="POST")
    req.add_header("Content-Type", "application/json")
    try:
        return urllib.request.urlopen(req).status
    except urllib.error.HTTPError as e:
        return e.code


def main():
    base = sys.argv[1] if len(sys.argv) > 1 else "http://localhost:8066"
    s = S(base)
    stamp = str(int(time.time()))
    a = diff.login(base, "bench0200", PASSWORD)
    b = diff.login(base, "bench0201", PASSWORD)
    me = s.call(a, "GET", "/users/me")[1]
    other = s.call(b, "GET", "/users/me")[1]
    team = s.call(a, "GET", "/users/me/teams")[1][0]["id"]
    ch = s.call(a, "POST", "/channels", {"team_id": team, "name": f"cache-{stamp}-{base[-4:]}",
                                         "display_name": f"Cache {stamp}", "type": "O"})[1]
    cid = ch["id"]

    # --- session: repeated requests from the cache; logout ends it at once.
    st = s.stats()
    t = diff.login(base, "bench0202", PASSWORD)
    s.call(t, "GET", "/users/me")
    s.call(t, "GET", "/users/me")
    s.counted("session", st, "hits", "session")
    s.call(t, "POST", "/users/logout")
    s.check("session: revoked token refused at once", s.call(t, "GET", "/users/me", ok=None)[0] == 401)

    # --- user profile: users/ids twice, patch, users/ids shows it.
    st = s.stats()
    s.call(a, "POST", "/users/ids", [other["id"]])
    s.call(a, "POST", "/users/ids", [other["id"]])
    s.counted("user", st, "hits", "profile")
    st = s.stats()
    s.call(b, "PUT", "/users/me/patch", {"nickname": f"nick{stamp}"})
    got = s.call(a, "POST", "/users/ids", [other["id"]])[1][0]["nickname"]
    s.check("profile: patch visible to others at once", got == f"nick{stamp}", got)
    s.counted("user", st, "invalidations", "profile")

    # --- all profiles: page 0 of 100 without filters.
    st = s.stats()
    s.call(a, "GET", "/users?page=0&per_page=100")
    s.call(a, "GET", "/users?page=0&per_page=100")
    s.counted("all_profiles", st, "hits", "all profiles")

    # --- status: status/ids twice, set dnd, status/ids shows it.
    st = s.stats()
    s.call(a, "POST", "/users/status/ids", [other["id"]])
    s.call(a, "POST", "/users/status/ids", [other["id"]])
    s.counted("status", st, "hits", "status")
    s.call(b, "PUT", "/users/me/status", {"user_id": other["id"], "status": "dnd"})
    got = s.call(a, "POST", "/users/status/ids", [other["id"]])[1][0]["status"]
    s.check("status: change visible at once", got == "dnd", got)
    s.call(b, "PUT", "/users/me/status", {"user_id": other["id"], "status": "online"})

    # --- channel by id, by name: patch the header and the display name.
    st = s.stats()
    s.call(a, "GET", f"/channels/{cid}")
    s.call(a, "GET", f"/channels/{cid}")
    s.counted("channel", st, "hits", "channel")
    s.call(a, "PUT", f"/channels/{cid}/patch", {"header": f"h{stamp}", "display_name": f"Renamed {stamp}"})
    s.check("channel: patch visible by id", s.call(a, "GET", f"/channels/{cid}")[1]["header"] == f"h{stamp}")
    got = s.call(a, "GET", f"/teams/{team}/channels/name/{ch['name']}")[1]["display_name"]
    s.check("channel: patch visible by name", got == f"Renamed {stamp}", got)

    # --- memberships (permission checks), member counts, profiles in channel.
    st = s.stats()
    s.call(a, "GET", f"/channels/{cid}/stats")
    s.call(a, "GET", f"/channels/{cid}/stats")
    s.counted("member_count", st, "hits", "member count")
    s.check("membership: not a member yet → no access to a private channel",
            s.call(b, "GET", f"/channels/{cid}/posts?per_page=5", ok=None)[0] in (200, 403))
    s.call(b, "POST", f"/channels/{cid}/members", {"user_id": other["id"]})
    got = s.call(a, "GET", f"/channels/{cid}/stats")[1]["member_count"]
    s.check("member count: join counted at once", got == 2, got)
    s.check("membership: the new member reads at once",
            s.call(b, "GET", f"/channels/{cid}/posts?per_page=5", ok=None)[0] == 200)
    priv = s.call(a, "POST", "/channels", {"team_id": team, "name": f"cachep-{stamp}-{base[-4:]}",
                                           "display_name": "P", "type": "P"})[1]
    s.call(a, "POST", f"/channels/{priv['id']}/members", {"user_id": other["id"]})
    s.check("membership: added to a private channel → reads at once",
            s.call(b, "GET", f"/channels/{priv['id']}", ok=None)[0] == 200)
    # A private channel's read checks the user's memberships (GetAllChannelMembersForUser).
    st = s.stats()
    s.call(b, "GET", f"/channels/{priv['id']}")
    s.call(b, "GET", f"/channels/{priv['id']}")
    s.counted("members_for_user", st, "hits", "memberships")
    s.call(a, "DELETE", f"/channels/{priv['id']}/members/{other['id']}")
    s.check("membership: removed → refused at once",
            s.call(b, "GET", f"/channels/{priv['id']}", ok=None)[0] == 403)

    # --- member roles (GetSchemeRolesForChannel → GetScheme): a promotion is visible at once.
    s.call(a, "PUT", f"/channels/{cid}/members/{other['id']}/roles", {"roles": "channel_user channel_admin"})
    got = s.call(b, "GET", f"/channels/{cid}/members/me")[1]
    s.check("member roles: promotion visible at once", got["scheme_admin"] is True and "channel_admin" in got["roles"],
            got["roles"])
    s.call(a, "PUT", f"/channels/{cid}/members/{other['id']}/roles", {"roles": "channel_user"})
    got = s.call(b, "GET", f"/channels/{cid}/members/me")[1]["scheme_admin"]
    s.check("member roles: demotion visible at once", got is False, got)
    code = s.call(a, "PUT", f"/channels/{cid}/members/{other['id']}/roles", {"roles": "system_admin"}, ok=None)[0]
    s.check("member roles: a system role refused", code == 400, code)

    # --- posts: since-cache ("nothing new"), then a new post shows.
    now = int(time.time() * 1000)
    st = s.stats()
    s.call(a, "GET", f"/channels/{cid}/posts?since={now}")
    s.call(a, "GET", f"/channels/{cid}/posts?since={now}")
    s.counted("last_post_time", st, "hits", "posts since")
    p = s.call(b, "POST", "/posts", {"channel_id": cid, "message": f"hello @{me['username']}",
                                     "pending_post_id": f"{other['id']}:{now}"})[1]
    got = s.call(a, "GET", f"/channels/{cid}/posts?since={now}")[1]["order"]
    s.check("posts since: the new post shows at once", p["id"] in got, got)
    again = s.call(b, "POST", "/posts", {"channel_id": cid, "message": f"hello @{me['username']}",
                                         "pending_post_id": f"{other['id']}:{now}"})[1]
    s.check("pending post id: a retry gives the same post", again["id"] == p["id"], again["id"])
    s.call(b, "POST", "/posts", {"channel_id": cid, "message": "second"})
    st = s.stats()
    s.call(b, "POST", "/posts", {"channel_id": cid, "message": "third"})
    s.counted("members_notify_props", st, "hits", "notify props")
    s.counted("profiles_in_channel", st, "hits", "profiles in channel")

    # --- notify props: muting must change what the next post does.
    s.call(a, "PUT", f"/channels/{cid}/members/me/notify_props", {"channel_id": cid, "user_id": me["id"],
                                                                  "mark_unread": "mention"})
    got = s.call(a, "GET", f"/channels/{cid}/members/me")[1]["notify_props"]["mark_unread"]
    s.check("notify props: change visible at once", got == "mention", got)

    # --- reactions and pins in post metadata and stats.
    s.call(b, "POST", "/reactions", {"user_id": other["id"], "post_id": p["id"], "emoji_name": "smile"})
    got = [r["emoji_name"] for r in s.call(a, "GET", f"/posts/{p['id']}")[1].get("metadata", {}).get("reactions", [])]
    s.check("reactions: new reaction in metadata at once", "smile" in got, got)
    # Reactions are read (and cached) for posts that have some.
    st = s.stats()
    s.call(a, "GET", f"/posts/{p['id']}")
    s.counted("reactions", st, "hits", "reactions")
    s.call(b, "POST", "/reactions", {"user_id": other["id"], "post_id": p["id"], "emoji_name": "+1"})
    got = [r["emoji_name"] for r in s.call(a, "GET", f"/posts/{p['id']}")[1].get("metadata", {}).get("reactions", [])]
    s.check("reactions: a second reaction in cached metadata at once", "+1" in got, got)
    before = s.call(a, "GET", f"/channels/{cid}/stats")[1]["pinnedpost_count"]
    s.call(a, "POST", f"/posts/{p['id']}/pin")
    got = s.call(a, "GET", f"/channels/{cid}/stats")[1]["pinnedpost_count"]
    s.check("pinned count: pin counted at once", got == before + 1, f"{before} → {got}")

    # --- link previews: a permalink preview is built once and cached (openGraph / link cache).
    site = s.call(a, "GET", "/config/client?format=old")[1]["SiteURL"]
    tname = s.call(a, "GET", f"/teams/{team}")[1]["name"]
    lp = s.call(a, "POST", "/posts", {"channel_id": cid, "message": f"see {site}/{tname}/pl/{p['id']}"})[1]
    s.call(a, "GET", f"/posts/{lp['id']}")
    st = s.stats()
    got = s.call(a, "GET", f"/posts/{lp['id']}")[1].get("metadata", {}).get("embeds", [])
    s.check("link preview: permalink embed", [e.get("type") for e in got] == ["permalink"], got)
    s.counted("open_graph", st, "hits", "link preview")

    # --- roles: permission checks read roles from the cache.
    st = s.stats()
    s.call(a, "POST", "/roles/names", ["channel_user"])
    s.call(a, "POST", "/roles/names", ["channel_user"])
    s.counted("role", st, "hits", "roles")
    s.counted("role_permissions", st, "hits", "role permissions (ChannelHigherScopedPermissions)")


    # --- files: a file's info in post metadata from the cache.
    import urllib.request
    boundary = "----cachecheck"
    body = (f"--{boundary}\r\nContent-Disposition: form-data; name=\"channel_id\"\r\n\r\n{cid}\r\n"
            f"--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"a.txt\"\r\n"
            f"Content-Type: text/plain\r\n\r\nhello\r\n--{boundary}--\r\n").encode()
    req = urllib.request.Request(base + "/api/v4/files", data=body, method="POST")
    req.add_header("Authorization", "Bearer " + a)
    req.add_header("Content-Type", f"multipart/form-data; boundary={boundary}")
    fid = json.loads(urllib.request.urlopen(req).read())["file_infos"][0]["id"]
    fp = s.call(a, "POST", "/posts", {"channel_id": cid, "message": "file", "file_ids": [fid]})[1]
    s.call(a, "GET", f"/posts/{fp['id']}")
    st = s.stats()
    s.call(a, "GET", f"/posts/{fp['id']}")
    s.counted("file_infos", st, "hits", "file infos")
    files = s.call(a, "GET", f"/posts/{fp['id']}")[1].get("metadata", {}).get("files", [])
    s.check("file infos: attached file in metadata", [f["id"] for f in files] == [fid], files)

    # --- custom emoji: a reaction with a new custom emoji works at once (the not-found entry is dropped).
    name = f"cc{stamp}"
    s.call(a, "POST", "/reactions", {"user_id": me["id"], "post_id": p["id"], "emoji_name": name}, ok=None)
    png = bytes.fromhex("89504e470d0a1a0a0000000d4948445200000001000000010806000000"
                        "1f15c4890000000d49444154789c6360000002000154a24f5d0000000049454e44ae426082")
    eb = (f"--{boundary}\r\nContent-Disposition: form-data; name=\"emoji\"\r\n\r\n"
          f"{json.dumps({'creator_id': s.call(s.admin, 'GET', '/users/me')[1]['id'], 'name': name})}\r\n"
          f"--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"e.png\"\r\n"
          f"Content-Type: image/png\r\n\r\n").encode() + png + f"\r\n--{boundary}--\r\n".encode()
    req = urllib.request.Request(base + "/api/v4/emoji", data=eb, method="POST")
    req.add_header("Authorization", "Bearer " + s.admin)
    req.add_header("Content-Type", f"multipart/form-data; boundary={boundary}")
    urllib.request.urlopen(req).read()
    st_code = s.call(a, "POST", "/reactions", {"user_id": me["id"], "post_id": p["id"], "emoji_name": name},
                     ok=None)[0]
    s.check("emoji: a new custom emoji usable at once", st_code == 200, st_code)

    # --- incoming webhook: cached by id; deleting it stops it at once.
    hook = s.call(s.admin, "POST", "/hooks/incoming", {"channel_id": cid, "display_name": "cache"})[1]
    hook_post(base, hook["id"], "from hook")
    st = s.stats()
    code = hook_post(base, hook["id"], "from hook")
    s.check("webhook: posts", code == 200, code)
    s.counted("webhook", st, "hits", "webhook")
    # A new post drops the channel from both channel caches (handlePostEvents → InvalidateCacheForChannel), so
    # the by-name cache serves reads between posts: GetOrCreateDirectChannel finds a DM by name through it.
    s.call(a, "POST", "/channels/direct", [me["id"], other["id"]])
    st = s.stats()
    dm = s.call(a, "POST", "/channels/direct", [me["id"], other["id"]])[1]
    s.counted("channel_by_name", st, "hits", "channel by name")
    s.call(a, "PUT", f"/channels/{dm['id']}/patch", {"header": f"dm{stamp}"})
    got = s.call(a, "POST", "/channels/direct", [me["id"], other["id"]])[1]["header"]
    s.check("channel by name: patch visible at once", got == f"dm{stamp}", got)
    s.call(s.admin, "DELETE", f"/hooks/incoming/{hook['id']}")
    code = hook_post(base, hook["id"], "after delete")
    s.check("webhook: deleted hook refused at once", code == 400, code)

    # --- clean up: the test channel is archived.
    s.call(a, "DELETE", f"/channels/{cid}")
    s.call(b, "PUT", "/users/me/patch", {"nickname": ""})

    if s.rust:
        print("\ncache        len    hits  misses  invalidations")
        for k, v in s.stats().items():
            print(f"{k:22} {v['len']:5} {v['hits']:7} {v['misses']:7} {v['invalidations']:7}")
    print(f"\n{s.n - s.fail}/{s.n} checks passed on {base}")
    sys.exit(1 if s.fail else 0)


if __name__ == "__main__":
    main()
