"""The core client scenario of sqldiff.py: the first load, preferences and status, channels, posts, threads, files,
drafts, sidebar, commands and webhooks, archiving, users and teams. A scenario module defines register(s), where s
carries what the steps need (see sqldiff.Scenario)."""

import json
import time
import urllib.error
import urllib.request


def register(s):
    step, call, keep, ctx = s.step, s.call, s.keep, s.ctx
    u1, u1_id, u2, u2_id, u3, u3_id = s.u1, s.u1_id, s.u2, s.u2_id, s.u3, s.u3_id
    admin, spare, spare_id = s.admin, s.spare, s.spare_id
    team, public, bench00, impl = s.team, s.public, s.bench00, s.impl
    BASE, PASSWORD = s.base, s.password

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

