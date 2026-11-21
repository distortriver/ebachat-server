"""What the stock web/desktop client and EBAChat read on the first load and afterwards, compared by value
(s.body: ids and timestamps normalized), for a regular user and for the admin. A thread with a mention is made
first so the thread, mention and unread reads have something to show."""

import json
import struct
import time
import zlib


def png():
    """A 1x1 PNG."""
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(b"\x00\xff\x00\x00")) + chunk(b"IEND", b""))


def multipart(fields, files):
    """fields {name: str}, files {name: (filename, bytes)} -> (body, content type)."""
    b = "sqldiffreadsboundary"
    out = b""
    for k, v in fields.items():
        out += f'--{b}\r\nContent-Disposition: form-data; name="{k}"\r\n\r\n{v}\r\n'.encode()
    for k, (fn, data) in files.items():
        out += (f'--{b}\r\nContent-Disposition: form-data; name="{k}"; filename="{fn}"\r\n'
                "Content-Type: image/png\r\n\r\n").encode() + data + b"\r\n"
    return out + f"--{b}--\r\n".encode(), f"multipart/form-data; boundary={b}"


def register(s):
    step, call, keep, ctx, body = s.step, s.call, s.keep, s.ctx, s.body
    u1, u1_id, u2, u2_id, u3, u3_id = s.u1, s.u1_id, s.u2, s.u2_id, s.u3, s.u3_id
    admin, admin_id, team, public, bench00 = s.admin, s.admin_id, s.team, s.public, s.bench00

    # Something to read: a root by u2 in bench-00, a reply by u3 mentioning u1 (u1 follows the thread), a DM.
    step("reads_setup_root")(lambda: keep("root", call(u2, "POST", "/posts", {"channel_id": bench00, "message": "root for reads"})))
    step("reads_setup_reply")(lambda: keep("reply", call(u3, "POST", "/posts", {
        "channel_id": bench00, "root_id": ctx["root"], "message": "@bench0101 have a look"})))
    step("reads_setup_dm")(lambda: keep("dm", call(u2, "POST", "/channels/direct", [u2_id, u1_id])))
    step("reads_setup_dm_post")(lambda: call(u2, "POST", "/posts", {"channel_id": ctx["dm"], "message": "psst"}))
    # A custom emoji, a profile attribute with u1's value, a flagged and pinned post, a reaction, a draft.
    def emoji():
        raw, ctype = multipart({"emoji": json.dumps({"name": "readsemoji", "creator_id": u2_id})},
                               {"image": ("e.png", png())})
        return call(u2, "POST", "/emoji", raw=raw, ctype=ctype)
    step("reads_setup_emoji")(emoji)
    step("reads_setup_cpa_field")(lambda: keep("cpa", call(admin, "POST", "/custom_profile_attributes/fields",
                                                           {"name": "Department", "type": "text", "attrs": {"visibility": "always"}})))
    step("reads_setup_cpa_value")(lambda: call(u1, "PATCH", "/custom_profile_attributes/values", {ctx["cpa"]: "Sales"}))
    step("reads_setup_flag")(lambda: call(u1, "PUT", f"/users/{u1_id}/preferences",
                                          [{"user_id": u1_id, "category": "flagged_post", "name": ctx["root"], "value": "true"}]))
    step("reads_setup_pin")(lambda: call(u2, "POST", f"/posts/{ctx['root']}/pin"))
    step("reads_setup_reaction")(lambda: call(u1, "POST", "/reactions",
                                              {"user_id": u1_id, "post_id": ctx["root"], "emoji_name": "smile"}))
    step("reads_setup_draft")(lambda: call(u1, "POST", "/drafts", {"user_id": u1_id, "channel_id": bench00, "message": "unsent"}))
    step("reads_settle")(lambda: (time.sleep(1), (200, None, None))[1])

    for who, tok, uid in (("u1", u1, u1_id), ("admin", admin, admin_id)):
        def r(name, method, path, data=None, unordered=(), tok=tok):
            step(f"reads_{who}_{name}")(lambda: body(call(tok, method, path, data), unordered))

        r("me", "GET", "/users/me")
        r("teams", "GET", "/users/me/teams", unordered=("",))
        r("team_members", "GET", "/users/me/teams/members", unordered=("",))
        r("teams_unread", "GET", "/users/me/teams/unread", unordered=("",))
        r("team_unread", "GET", f"/users/{uid}/teams/{team}/unread")
        r("channels", "GET", f"/users/me/teams/{team}/channels?include_deleted=true", unordered=("",))
        r("channel_members", "GET", f"/users/me/teams/{team}/channels/members", unordered=("",))
        r("all_channels", "GET", "/users/me/channels?include_deleted=true&last_delete_at=0", unordered=("",))
        r("all_channel_members", "GET", f"/users/{uid}/channel_members?page=0&per_page=200", unordered=("",))
        r("categories", "GET", f"/users/{uid}/teams/{team}/channels/categories")
        r("preferences", "GET", "/users/me/preferences", unordered=("",))
        r("config_client", "GET", "/config/client?format=old")
        r("license_client", "GET", "/license/client?format=old")
        r("roles_names", "POST", "/roles/names", ["system_user", "system_admin", "team_user", "team_admin",
                                                  "channel_user", "channel_admin", "system_guest"], unordered=("",))
        r("ping", "GET", "/system/ping?get_server_status=true")
        r("status_ids", "POST", "/users/status/ids", [u1_id, u2_id, u3_id, admin_id], unordered=("",))
        r("users_ids", "POST", "/users/ids", [u1_id, u2_id, u3_id], unordered=("",))
        r("users_in_channel", "GET", f"/users?in_channel={bench00}&page=0&per_page=100&sort=admin")
        r("users_in_team", "GET", f"/users?in_team={team}&page=0&per_page=30")
        r("channel_posts", "GET", f"/channels/{bench00}/posts?page=0&per_page=30")
        r("posts_unread", "GET", f"/users/{uid}/channels/{bench00}/posts/unread?limit_before=30&limit_after=30&skipFetchThreads=false&collapsedThreads=true&collapsedThreadsExtended=false")
        r("channel_unread", "GET", f"/users/{uid}/channels/{bench00}/unread")
        r("channel_stats", "GET", f"/channels/{bench00}/stats?exclude_files_count=true")
        r("channel_member_me", "GET", f"/channels/{bench00}/members/me")
        step(f"reads_{who}_thread")(lambda tok=tok: body(call(tok, "GET", f"/posts/{ctx['root']}/thread?skipFetchThreads=false&collapsedThreads=true&collapsedThreadsExtended=false&direction=down&perPage=60")))
        r("threads", "GET", f"/users/{uid}/teams/{team}/threads?per_page=25&extended=true")
        r("threads_totals", "GET", f"/users/{uid}/teams/{team}/threads?totalsOnly=true&threadsOnly=false")
        r("threads_unread", "GET", f"/users/{uid}/teams/{team}/threads?unread=true&per_page=25")
        r("emoji", "GET", "/emoji?page=0&per_page=200&sort=name")
        r("plugins_webapp", "GET", "/plugins/webapp")
        r("plugin_statuses", "GET", "/plugins/statuses")
        r("terms", "GET", "/terms_of_service")
        r("groups", "GET", "/groups?page=0&per_page=60&include_member_count=true")
        r("cpa_fields", "GET", "/custom_profile_attributes/fields")
        r("cpa_values", "GET", f"/users/{uid}/custom_profile_attributes")
        r("mentions_search", "POST", f"/teams/{team}/posts/search",
          {"terms": "@bench0101 ", "is_or_search": True, "include_deleted_channels": True, "time_zone_offset": 0,
           "page": 0, "per_page": 20})
        r("mentions_search_all", "POST", "/posts/search",
          {"terms": "@bench0101 ", "is_or_search": True, "include_deleted_channels": True, "time_zone_offset": 0,
           "page": 0, "per_page": 20})
        r("flagged", "GET", f"/users/{uid}/posts/flagged?page=0&per_page=20")
        r("pinned", "GET", f"/channels/{bench00}/pinned")
        r("drafts", "GET", f"/users/{uid}/teams/{team}/drafts")
        r("scheduled", "GET", f"/posts/scheduled/team/{team}?includeDirectChannels=true")
        r("team", "GET", f"/teams/{team}")
        r("team_stats", "GET", f"/teams/{team}/stats")
        r("my_status", "GET", f"/users/{uid}/status")
        r("commands_autocomplete", "GET", f"/teams/{team}/commands/autocomplete_suggestions?user_input=%2F&channel_id={bench00}&team_id={team}")
        r("data_retention", "GET", "/data_retention/policy")
        r("user_terms", "GET", f"/users/{uid}/terms_of_service")
        r("limits", "GET", "/limits/server")
        r("bots", "GET", "/bots?include_deleted=true&page=0&per_page=200")
