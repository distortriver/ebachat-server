"""Live interaction scenario of sqldiff.py: websocket client actions (typing, statuses, activity), status changes,
built-in and custom slash commands, incoming and outgoing webhooks, channel search/autocomplete/by name/moderations,
DM/GM creation and its preferences, GM to private channel conversion.

Custom commands and outgoing webhooks call a small HTTP server this module starts on a free port (one per run).
Where an answer's values matter (statuses, command replies), the step returns them folded into the keys, since the
tool records keys only."""

import http.server
import json
import queue
import re
import threading
import time
import urllib.error
import urllib.parse
import urllib.request


class Hook(http.server.BaseHTTPRequestHandler):
    calls = []

    def do_POST(self):  # noqa: N802
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode()
        Hook.calls.append((self.command, self.path, self.headers.get("Content-Type"), body))
        self.reply()

    def do_GET(self):  # noqa: N802
        Hook.calls.append((self.command, self.path, None, ""))
        self.reply()

    def reply(self):
        if self.path.startswith("/out"):
            out = {"text": "outgoing reply", "username": "outbot"}
        else:
            out = {"response_type": "in_channel", "text": "custom reply", "username": "cmdbot",
                   "props": {"from": "live"}}
        data = json.dumps(out).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *a):
        pass


def flat(r, *fields):
    """The answer with the given fields' values folded into its keys: f=value."""
    s, v, h = r
    if isinstance(v, dict) and s < 300:
        # the server's own port shows in its links
        return s, {re.sub(r"localhost:\d+", "localhost", f"{f}={json.dumps(v.get(f), sort_keys=True)}"): 1
                   for f in fields}, h
    return r


def register(s):
    step, call, keep, ctx, psql = s.step, s.call, s.keep, s.ctx, s.psql
    u1, u1_id, u2, u2_id, u3, u3_id = s.u1, s.u1_id, s.u2, s.u2_id, s.u3, s.u3_id
    admin, spare_id, team = s.admin, s.spare_id, s.team
    BASE = s.base

    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Hook)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    hook_url = f"http://127.0.0.1:{srv.server_address[1]}"
    # Integrations reach the local server, and webhooks may override name and icon.
    st, v, _ = call(admin, "PUT", "/config/patch", {"ServiceSettings": {
        "AllowedUntrustedInternalConnections": "127.0.0.1 localhost",
        "EnablePostUsernameOverride": True, "EnablePostIconOverride": True,
        "EnableCommands": True, "EnableIncomingWebhooks": True, "EnableOutgoingWebhooks": True}})
    assert st == 200, (st, v)
    ids = dict(line.split("\t") for line in psql(
        "SELECT Username, Id FROM Users WHERE Username IN ('bench0104','bench0106','bench0107')").splitlines())
    u6, u7 = ids["bench0106"], ids["bench0107"]

    # ---- a socket of u3's that sends actions ----
    sock = {}

    def ws_connect():
        import websocket
        sock["ws"] = websocket.create_connection(BASE.replace("http", "ws") + "/api/v4/websocket",
                                                 header=["Authorization: Bearer " + u3], suppress_origin=True)
        sock["seq"], sock["replies"] = 0, queue.Queue()
        threading.Thread(target=reader, daemon=True).start()
        return 200, None, None

    def reader():
        """Reads all the time, as a client does: pings get their pongs, replies go to the queue."""
        while True:
            try:
                m = json.loads(sock["ws"].recv())
            except Exception:  # noqa: BLE001
                return
            if "seq_reply" in m:
                sock["replies"].put(m)

    def ws(action, data):
        sock["seq"] += 1
        seq = sock["seq"]
        sock["ws"].send(json.dumps({"action": action, "seq": seq, "data": data}))
        try:
            while True:
                m = sock["replies"].get(timeout=10)
                if m.get("seq_reply") == seq:
                    break
        except queue.Empty:
            return "timeout", None, None
        status = m.get("status")
        out = {"status=" + str(status): 1}
        if isinstance(m.get("data"), dict):
            for k, val in sorted(m["data"].items()):
                # statuses: id -> status; keep the ones of the users here
                if k in (u1_id, u2_id, u3_id):
                    out[f"{k[:6]}={val}"] = 1
                elif len(k) != 26:
                    out[f"data.{k}"] = 1
        if m.get("error"):
            out[f"error={m['error'].get('id')} {m['error'].get('status_code')}"] = 1
        return (200 if status == "OK" else 400), out, None

    def last_post(channel=None):
        s_, v, h = call(u1, "GET", f"/channels/{channel or ctx['ch']}/posts?page=0&per_page=1")
        if s_ != 200 or not v["order"]:
            return s_, v, h
        p = dict(v["posts"][v["order"][0]])
        p["props"] = {k: val for k, val in sorted(p.get("props", {}).items())}
        return flat((s_, p, h), "message", "props", "type", "root_id")

    def rows():
        """The statuses of u1..u3 as stored."""
        return 200, {"row=" + r: 1 for r in psql(
            f"SELECT left(userid, 6), status, manual FROM status WHERE userid IN ('{u1_id}','{u2_id}','{u3_id}')"
            " ORDER BY 1").splitlines()}, None

    def status_of(uid):
        return flat(call(u1, "GET", f"/users/{uid}/status"), "status", "manual")

    # ---- a channel to work in ----
    step("live_channel")(lambda: keep("ch", call(u1, "POST", "/channels",
                                                  {"team_id": team, "name": "live-chan", "display_name": "Live", "type": "O"})))
    step("live_members")(lambda: call(u1, "POST", f"/channels/{ctx['ch']}/members", {"user_ids": [u2_id, u3_id]}))
    step("live_root")(lambda: keep("root", call(u1, "POST", "/posts", {"channel_id": ctx["ch"], "message": "root of a thread"})))

    # ---- websocket actions ----
    step("ws_connect")(ws_connect)
    step("ws_typing")(lambda: ws("user_typing", {"channel_id": ctx["ch"]}))
    step("ws_typing_thread")(lambda: ws("user_typing", {"channel_id": ctx["ch"], "parent_id": ctx["root"]}))
    step("ws_typing_bad")(lambda: ws("user_typing", {"channel_id": "x" * 26}))
    step("ws_get_statuses")(lambda: ws("get_statuses", {}))
    step("ws_statuses_ids")(lambda: ws("get_statuses_by_ids", {"user_ids": [u1_id, u2_id, u3_id]}))
    step("ws_statuses_ids_bad")(lambda: ws("get_statuses_by_ids", {"user_ids": ["bad"]}))
    step("ws_inactive_auto")(lambda: ws("user_update_active_status", {"user_is_active": False, "manual": False}))
    step("ws_inactive_auto_status")(lambda: status_of(u3_id))
    step("ws_inactive_manual")(lambda: ws("user_update_active_status", {"user_is_active": False, "manual": True}))
    step("ws_inactive_manual_status")(lambda: status_of(u3_id))
    step("ws_active_auto")(lambda: ws("user_update_active_status", {"user_is_active": True, "manual": False}))
    step("ws_active_auto_status")(lambda: status_of(u3_id))
    step("ws_active_manual")(lambda: ws("user_update_active_status", {"user_is_active": True, "manual": True}))
    step("ws_active_manual_status")(lambda: status_of(u3_id))
    step("ws_unknown_action")(lambda: ws("no_such_action", {}))

    # ---- status changes over REST ----
    for st_ in ("away", "dnd", "offline", "online"):
        step(f"status_{st_}")(lambda st_=st_: call(u3, "PUT", f"/users/{u3_id}/status", {"user_id": u3_id, "status": st_}))
        step(f"status_{st_}_get")(lambda: status_of(u3_id))
    step("status_dnd_timed")(lambda: call(u3, "PUT", f"/users/{u3_id}/status",
                                          {"user_id": u3_id, "status": "dnd", "dnd_end_time": int(time.time()) + 7200}))
    step("status_dnd_timed_get")(lambda: status_of(u3_id))
    step("status_bad")(lambda: call(u3, "PUT", f"/users/{u3_id}/status", {"user_id": u3_id, "status": "busy"}))
    step("status_other_user")(lambda: call(u2, "PUT", f"/users/{u3_id}/status", {"user_id": u3_id, "status": "away"}))
    step("status_admin_sets")(lambda: call(admin, "PUT", f"/users/{u3_id}/status", {"user_id": u3_id, "status": "online"}))
    step("status_admin_sets_get")(lambda: status_of(u3_id))
    step("status_ids")(lambda: call(u1, "POST", "/users/status/ids", [u1_id, u2_id, u3_id]))

    # ---- built-in slash commands ----
    def cmd(token, channel, text):
        return flat(call(token, "POST", "/commands/execute", {"channel_id": channel, "team_id": team, "command": text}),
                    "response_type", "text", "goto_location", "username", "icon_url", "props")

    step("rows_before_other_channel")(rows)
    step("other_channel")(lambda: keep("other", call(u1, "POST", "/channels",
                                                     {"team_id": team, "name": "live-other", "display_name": "Other", "type": "O"})))
    step("cmd_join")(lambda: cmd(u3, ctx["ch"], "/join ~live-other"))
    step("cmd_join_again")(lambda: cmd(u3, ctx["ch"], "/join ~live-other"))
    step("cmd_join_missing")(lambda: cmd(u3, ctx["ch"], "/join ~no-such-channel"))
    step("cmd_leave")(lambda: cmd(u3, ctx["other"], "/leave"))
    step("cmd_msg")(lambda: cmd(u3, ctx["ch"], "/msg @bench0104 hello there"))
    step("cmd_msg_prefs")(lambda: call(u3, "GET", "/users/me/preferences/direct_channel_show"))
    step("cmd_msg_self")(lambda: cmd(u3, ctx["ch"], "/msg @bench0103 note"))
    step("cmd_groupmsg")(lambda: cmd(u3, ctx["ch"], "/groupmsg @bench0104,@bench0101 hello group"))
    step("cmd_groupmsg_prefs")(lambda: call(u3, "GET", "/users/me/preferences/group_channel_show"))
    step("cmd_invite")(lambda: cmd(u1, ctx["ch"], "/invite @bench0104"))
    step("cmd_invite_post")(lambda: last_post())
    step("cmd_invite_other")(lambda: cmd(u1, ctx["ch"], "/invite @bench0102 ~live-other"))
    step("cmd_invite_already")(lambda: cmd(u1, ctx["ch"], "/invite @bench0104"))
    step("cmd_kick")(lambda: cmd(u1, ctx["ch"], "/kick @bench0104"))
    step("cmd_kick_post")(lambda: last_post())
    step("cmd_kick_not_member")(lambda: cmd(u1, ctx["ch"], "/kick @bench0104"))
    step("cmd_remove")(lambda: cmd(u1, ctx["other"], "/remove @bench0102"))
    step("cmd_header")(lambda: cmd(u1, ctx["ch"], "/header a new header"))
    step("cmd_header_post")(lambda: last_post())
    step("cmd_purpose")(lambda: cmd(u1, ctx["ch"], "/purpose a new purpose"))
    step("cmd_purpose_post")(lambda: last_post())
    step("cmd_rename")(lambda: cmd(u1, ctx["ch"], "/rename Live Renamed"))
    step("cmd_rename_post")(lambda: last_post())
    step("cmd_mute")(lambda: cmd(u2, ctx["ch"], "/mute"))
    step("cmd_mute_get")(lambda: flat(call(u2, "GET", f"/channels/{ctx['ch']}/members/me"), "notify_props"))
    step("cmd_unmute")(lambda: cmd(u2, ctx["ch"], "/mute"))
    step("cmd_mute_other")(lambda: cmd(u2, ctx["ch"], "/mute ~live-other"))
    step("cmd_echo")(lambda: cmd(u1, ctx["ch"], '/echo "echoed text" 0'))
    step("cmd_echo_post")(lambda: last_post())
    step("cmd_code")(lambda: cmd(u1, ctx["ch"], "/code let x = 1;"))
    step("cmd_code_post")(lambda: last_post())
    step("cmd_search")(lambda: flat(cmd(u1, ctx["ch"], "/search root"), "response_type", "text", "goto_location"))
    step("cmd_settings")(lambda: flat(cmd(u1, ctx["ch"], "/settings"), "response_type", "text", "goto_location"))
    step("cmd_unknown")(lambda: cmd(u1, ctx["ch"], "/nosuchcommand"))
    step("cmd_list")(lambda: call(u1, "GET", f"/commands?team_id={team}"))
    step("cmd_autocomplete")(lambda: call(u1, "GET", f"/teams/{team}/commands/autocomplete"))
    step("cmd_suggestions")(lambda: call(u1, "GET", f"/teams/{team}/commands/autocomplete_suggestions?user_input=/he"
                                                    f"&channel_id={ctx['ch']}"))

    # ---- custom slash command ----
    step("rows_before_custom_create")(rows)
    step("custom_create")(lambda: keep("cmd", call(admin, "POST", "/commands", {
        "team_id": team, "trigger": "livecmd", "method": "P", "url": hook_url + "/cmd", "display_name": "Live",
        "description": "d", "auto_complete": True, "auto_complete_desc": "live", "auto_complete_hint": "[x]",
        "username": "cmduser", "icon_url": "http://example.com/i.png"})))
    step("custom_get")(lambda: call(admin, "GET", f"/commands/{ctx['cmd']}"))
    step("custom_list")(lambda: call(admin, "GET", f"/commands?team_id={team}&custom_only=true"))
    step("custom_execute")(lambda: flat(cmd(u1, ctx["ch"], "/livecmd some args"), "response_type", "text"))
    step("custom_execute_post")(lambda: last_post())

    def custom_update():
        c = call(admin, "GET", f"/commands/{ctx['cmd']}")[1]
        c["method"], c["display_name"] = "G", "Live 2"
        return call(admin, "PUT", f"/commands/{ctx['cmd']}", c)

    step("custom_update")(custom_update)
    step("custom_execute_get")(lambda: flat(cmd(u1, ctx["ch"], "/livecmd again"), "response_type", "text"))
    step("custom_execute_get_post")(lambda: last_post())
    step("custom_regen")(lambda: call(admin, "PUT", f"/commands/{ctx['cmd']}/regen_token"))
    step("custom_move_bad")(lambda: call(admin, "PUT", f"/commands/{ctx['cmd']}/move", {"team_id": "x" * 26}))
    step("custom_delete")(lambda: call(admin, "DELETE", f"/commands/{ctx['cmd']}"))
    step("custom_execute_gone")(lambda: cmd(u1, ctx["ch"], "/livecmd gone"))

    # ---- incoming webhook ----
    def incoming(hook, payload):
        req = urllib.request.Request(BASE + "/hooks/" + hook, data=json.dumps(payload).encode(),
                                     method="POST", headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                return r.status, None, r.headers
        except urllib.error.HTTPError as e:
            return e.code, None, e.headers

    step("rows_before_inhook_create")(rows)
    step("inhook_create")(lambda: keep("inhook", call(admin, "POST", "/hooks/incoming", {
        "channel_id": ctx["ch"], "display_name": "live in", "description": "d", "username": "hookdefault"})))
    step("inhook_post_override")(lambda: incoming(ctx["inhook"], {
        "text": "hook with override", "username": "overridden", "icon_url": "http://example.com/a.png",
        "props": {"card": "a card"}}))
    step("inhook_post_override_post")(lambda: last_post())
    step("inhook_post_attachments")(lambda: incoming(ctx["inhook"], {
        "attachments": [{"fallback": "fb", "color": "#ff0000", "pretext": "pre", "text": "attached",
                         "title": "T", "fields": [{"title": "f", "value": "v", "short": True}]}]}))
    step("inhook_post_attachments_post")(lambda: last_post())
    step("inhook_post_channel")(lambda: incoming(ctx["inhook"], {"text": "to other", "channel": "live-other"}))
    step("inhook_post_channel_post")(lambda: last_post(ctx["other"]))
    step("inhook_post_form")(lambda: incoming_form(ctx["inhook"]))

    def incoming_form(hook):
        data = urllib.parse.urlencode({"payload": json.dumps({"text": "form payload"})}).encode()
        req = urllib.request.Request(BASE + "/hooks/" + hook, data=data, method="POST",
                                     headers={"Content-Type": "application/x-www-form-urlencoded"})
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                return r.status, None, r.headers
        except urllib.error.HTTPError as e:
            return e.code, None, e.headers

    step("inhook_post_empty")(lambda: incoming(ctx["inhook"], {}))
    step("inhook_last_post")(lambda: last_post())

    step("inhook_delete")(lambda: call(admin, "DELETE", f"/hooks/incoming/{ctx['inhook']}"))

    # ---- outgoing webhook ----
    step("rows_before_outhook_create")(rows)
    step("outhook_create")(lambda: keep("outhook", call(admin, "POST", "/hooks/outgoing", {
        "team_id": team, "channel_id": ctx["ch"], "display_name": "live out", "trigger_words": ["livebang"],
        "trigger_when": 0, "callback_urls": [hook_url + "/out"], "content_type": "application/json"})))
    step("outhook_list")(lambda: call(admin, "GET", f"/hooks/outgoing?team_id={team}"))
    step("outhook_fire")(lambda: call(u1, "POST", "/posts", {"channel_id": ctx["ch"], "message": "livebang now"}))
    step("outhook_reply")(lambda: (time.sleep(1), last_post())[1])
    step("outhook_nofire")(lambda: call(u1, "POST", "/posts", {"channel_id": ctx["ch"], "message": "no trigger here"}))
    step("outhook_regen")(lambda: call(admin, "POST", f"/hooks/outgoing/{ctx['outhook']}/regen_token"))
    step("rows_before_outhook_update")(rows)
    step("outhook_update")(lambda: call(admin, "PUT", f"/hooks/outgoing/{ctx['outhook']}", {
        "id": ctx["outhook"], "team_id": team, "channel_id": ctx["ch"], "display_name": "live out 2",
        "trigger_words": ["livebang2"], "trigger_when": 1, "callback_urls": [hook_url + "/out"],
        "content_type": "application/x-www-form-urlencoded"}))
    step("outhook_fire_prefix")(lambda: call(u1, "POST", "/posts", {"channel_id": ctx["ch"], "message": "livebang2xyz form"}))
    step("outhook_reply_prefix")(lambda: (time.sleep(1), last_post())[1])
    step("outhook_delete")(lambda: call(admin, "DELETE", f"/hooks/outgoing/{ctx['outhook']}"))

    # ---- channel search, autocomplete, by name, moderations ----
    step("rows_before_ch_search")(rows)
    step("ch_search")(lambda: call(u1, "POST", f"/teams/{team}/channels/search", {"term": "live"}))
    step("ch_search_private")(lambda: call(u1, "POST", f"/teams/{team}/channels/private/search", {"term": "live"}))
    step("ch_search_all")(lambda: call(admin, "POST", "/channels/search?system_console=false", {"term": "live"}))
    step("ch_search_all_paged")(lambda: call(admin, "POST", "/channels/search",
                                             {"term": "live", "page": 0, "per_page": 10}))
    step("ch_search_group")(lambda: call(u3, "POST", "/channels/group/search", {"term": "bench0104"}))
    step("ch_autocomplete")(lambda: call(u1, "GET", f"/teams/{team}/channels/autocomplete?name=live"))
    step("ch_search_autocomplete")(lambda: call(u1, "GET", f"/teams/{team}/channels/search_autocomplete?name=live"))
    step("ch_by_name")(lambda: call(u1, "GET", f"/teams/{team}/channels/name/live-chan"))
    step("ch_by_name_missing")(lambda: call(u1, "GET", f"/teams/{team}/channels/name/no-such"))
    step("ch_by_team_name")(lambda: call(u1, "GET", "/teams/name/bench/channels/name/live-chan"))
    step("ch_moderations")(lambda: call(admin, "GET", f"/channels/{ctx['ch']}/moderations"))
    step("ch_moderations_user")(lambda: call(u1, "GET", f"/channels/{ctx['ch']}/moderations"))

    # ---- DM / GM and the preferences they write ----
    step("rows_before_dm_create")(rows)
    step("dm_create")(lambda: keep("dm", call(u3, "POST", "/channels/direct", [u3_id, u6])))
    step("dm_create_again")(lambda: call(u3, "POST", "/channels/direct", [u3_id, u6]))
    step("dm_self")(lambda: call(u3, "POST", "/channels/direct", [u3_id, u3_id]))
    step("dm_prefs")(lambda: call(u3, "GET", "/users/me/preferences/direct_channel_show"))
    step("gm_create")(lambda: keep("gm", call(u3, "POST", "/channels/group", [u3_id, u6, u7])))
    step("gm_create_again")(lambda: call(u3, "POST", "/channels/group", [u3_id, u6, u7]))
    step("gm_prefs")(lambda: call(u3, "GET", "/users/me/preferences/group_channel_show"))
    step("rows_before_gm_post")(rows)
    step("gm_post")(lambda: call(u3, "POST", "/posts", {"channel_id": ctx["gm"], "message": "gm hello"}))
    step("gm_convert_bad_team")(lambda: call(u3, "POST", f"/channels/{ctx['gm']}/convert_to_channel", {
        "channel_id": ctx["gm"], "team_id": "x" * 26, "name": "live-gm", "display_name": "Live GM"}))
    step("gm_convert")(lambda: flat(call(u3, "POST", f"/channels/{ctx['gm']}/convert_to_channel", {
        "channel_id": ctx["gm"], "team_id": team, "name": "live-gm", "display_name": "Live GM"}), "type", "name"))
    step("gm_convert_get")(lambda: flat(call(u3, "GET", f"/channels/{ctx['gm']}"), "type", "name", "team_id"))
    step("dm_convert")(lambda: call(u3, "POST", f"/channels/{ctx['dm']}/convert_to_channel", {
        "channel_id": ctx["dm"], "team_id": team, "name": "live-dm", "display_name": "Live DM"}))
    step("status_before_close")(lambda: status_of(u3_id))
    step("ws_close")(lambda: (sock["ws"].close(), (200, None, None))[1])
    step("ws_closed_status")(lambda: (time.sleep(2), status_of(u3_id))[1])
    step("ws_closed_rows")(rows)

    # A client that never reads (no pongs) is dropped after pongWaitTime (100 s) and goes offline.
    def silent():
        import websocket
        w = websocket.create_connection(BASE.replace("http", "ws") + "/api/v4/websocket",
                                        header=["Authorization: Bearer " + s.spare], suppress_origin=True)
        time.sleep(2)
        before = status_of(spare_id)[1]
        time.sleep(108)
        after = status_of(spare_id)[1]
        w.close()
        return 200, {**{"before." + k: 1 for k in before}, **{"after." + k: 1 for k in after}}, None

    step("ws_no_pong")(silent)
