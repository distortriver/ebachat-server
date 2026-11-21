"""Search and notifications scenario of sqldiff.py.

Search: post search in the client's syntax (words, phrases, from:/in:/before:/after:/on:, -excluded, hashtags,
@mentions, prefix*, is_or_search, include_deleted_channels, time_zone_offset, paging), recent mentions and user
search, compared by value (which posts, in which order).

Notifications: email goes to a small SMTP sink this module starts on a free port (one per run; the servers are
pointed at it with PUT /config/patch, and SERVER_ENV turns SendEmailNotifications on, which env.sh pins off). Each
notifying step is followed by a *_seen step whose answer is what came of it, compared by value: the mails (recipient,
subject and the headers that are not per-message), the websocket `posted` events each recipient got (their
mentions/followers fields) and the recipients' mention counts. Push notifications stay off: nothing may be written
for them (the writes of the notifying steps show it)."""

import email
import email.policy
import json
import re
import socket
import threading
import time

SERVER_ENV = {"MM_EMAILSETTINGS_SENDEMAILNOTIFICATIONS": "true"}

# Recipients: bench0111..0116, each in its own state when the notifying posts are made.
RECIPIENTS = ["bench0111", "bench0112", "bench0113", "bench0114", "bench0115", "bench0116"]


class Sink:
    """A minimal SMTP server: records (envelope recipients, headers) of each message."""

    def __init__(self):
        self.mails, self.lock = [], threading.Lock()
        self.srv = socket.socket()
        self.srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.srv.bind(("127.0.0.1", 0))
        self.srv.listen(16)
        self.port = self.srv.getsockname()[1]
        threading.Thread(target=self.accept, daemon=True).start()

    def accept(self):
        while True:
            c, _ = self.srv.accept()
            threading.Thread(target=self.session, args=(c,), daemon=True).start()

    def session(self, c):
        f = c.makefile("rb")

        def say(line):
            c.sendall(line.encode() + b"\r\n")
        try:
            say("220 sink ESMTP")
            rcpt, data = [], None
            while True:
                line = f.readline()
                if not line:
                    return
                cmd = line.decode(errors="replace").strip()
                up = cmd.upper()
                if data is not None:
                    if cmd == ".":
                        self.record(rcpt, b"".join(data))
                        rcpt, data = [], None
                        say("250 ok")
                    else:
                        data.append(line[1:] if line.startswith(b"..") else line)
                elif up.startswith("EHLO"):
                    c.sendall(b"250-sink\r\n250-8BITMIME\r\n250 SMTPUTF8\r\n")
                elif up.startswith("HELO") or up.startswith("MAIL") or up.startswith("RSET") or up.startswith("NOOP"):
                    if up.startswith("MAIL") or up.startswith("RSET"):
                        rcpt = []
                    say("250 ok")
                elif up.startswith("RCPT"):
                    rcpt.append(cmd.split(":", 1)[1].strip().strip("<>").split(">")[0])
                    say("250 ok")
                elif up == "DATA":
                    data = []
                    say("354 go ahead")
                elif up == "QUIT":
                    say("221 bye")
                    return
                else:
                    say("502 not implemented")
        except OSError:
            return
        finally:
            c.close()

    def record(self, rcpt, raw):
        msg = email.message_from_bytes(raw, policy=email.policy.default)
        headers = {k.lower(): str(v) for k, v in msg.items()}
        plain = msg.get_body(("plain",))
        text = plain.get_content() if plain is not None else None
        parts = sorted(p.get_content_type() for p in msg.walk())
        with self.lock:
            self.mails.append({"rcpt": sorted(rcpt), "headers": headers, "text": text, "parts": parts})

    def take(self):
        with self.lock:
            out, self.mails = self.mails, []
        return out


# Per-message or per-server header values: their presence is compared, not their value.
VOLATILE = {"message-id", "date"}


def mail_view(m):
    h = {}
    for k, v in m["headers"].items():
        if k in VOLATILE:
            v = "<present>"
        elif k == "content-type":
            v = re.sub(r'boundary="?[^";]+"?', "boundary=<b>", v)
        elif k == "subject":
            # the date in a direct message's subject is the server's local date
            v = re.sub(r" on \w+ \d+, \d{4}$", " on <date>", v)
        h[k] = re.sub(r"localhost:\d+", "localhost", v)
    text = m["text"]
    if text is not None:
        text = re.sub(r"localhost:\d+", "localhost", text)
        # times of day are the server's clock
        text = re.sub(r"\d{1,2}:\d{2}( [AP]M)?", "<time>", text)
        text = [line.strip() for line in text.splitlines() if line.strip()]
    return {"rcpt": m["rcpt"], "headers": h, "text": text, "parts": m["parts"]}


class Listener:
    """A recipient's websocket: keeps the data of its posted events."""

    def __init__(self, base, token):
        import websocket
        self.events, self.lock = [], threading.Lock()
        self.ws = websocket.create_connection(base.replace("http", "ws") + "/api/v4/websocket",
                                              header=["Authorization: Bearer " + token], suppress_origin=True)
        threading.Thread(target=self.read, daemon=True).start()

    def read(self):
        while True:
            try:
                m = json.loads(self.ws.recv())
            except Exception:  # noqa: BLE001
                return
            if m.get("event") in ("posted", "ephemeral_message"):
                d = m.get("data", {})
                post = json.loads(d.get("post", "{}"))
                with self.lock:
                    self.events.append({"event": m["event"]} | {k: d.get(k) for k in ("mentions", "followers", "channel_type", "set_online",
                                                                 "sender_name", "channel_display_name")}
                                       | {"message": post.get("message")})

    def take(self):
        with self.lock:
            out, self.events = self.events, []
        return out


def register(s):
    step, call, keep, ctx, body = s.step, s.call, s.keep, s.ctx, s.body
    u1, u1_id, u2, u2_id = s.u1, s.u1_id, s.u2, s.u2_id
    admin, team = s.admin, s.team
    users = {}  # username -> (token, id)
    listeners = {}
    sink = Sink()

    # ---- search: posts made for it, in a channel of their own ----
    def setup_search():
        r = keep("sch", call(admin, "POST", "/channels", {"team_id": team, "name": "notify-search",
                                                          "display_name": "Notify search", "type": "O"}))
        call(admin, "POST", f"/channels/{ctx['sch']}/members", {"user_ids": [u1_id, u2_id]})
        return r
    step("notify_search_setup")(setup_search)
    texts = [
        (u1, "zebrafish swim in the aquarium"),
        (u2, "the zebrafish genome is small"),
        (u1, "aquarium cleaning schedule #maintenance"),
        (u2, "a zebra crossing near the aquarium"),
        (u1, "@bench0102 look at the zebrafish"),
        (u2, "\"quoted zebrafish phrase\" inside"),
        (u1, "under_score and dash-word tokens"),
        (u2, "#maintenance done on the tank"),
    ]
    for i, (tok, msg) in enumerate(texts):
        step(f"notify_search_post{i}")(lambda tok=tok, msg=msg, i=i: keep(f"sp{i}", call(
            tok, "POST", "/posts", {"channel_id": ctx["sch"], "message": msg})))
    step("notify_search_dm")(lambda: call(u1, "POST", "/posts", {
        "channel_id": call(u1, "POST", "/channels/direct", [u1_id, u2_id])[1]["id"],
        "message": "secretword between us 42"}))
    step("notify_search_reply")(lambda: call(u1, "POST", "/posts", {"channel_id": ctx["sch"], "root_id": ctx["sp1"],
                                                                     "message": "zebrafish reply in thread"}))
    # an archived channel with a matching post, for include_deleted_channels
    step("notify_search_archived_setup")(lambda: keep("sarch", call(admin, "POST", "/channels", {
        "team_id": team, "name": "notify-archived", "display_name": "Notify archived", "type": "O"})))
    step("notify_search_archived_post")(lambda: (call(admin, "POST", f"/channels/{ctx['sarch']}/members",
                                                      {"user_id": u1_id}),
                                                 call(u1, "POST", "/posts", {"channel_id": ctx["sarch"],
                                                                             "message": "zebrafish in archive"}))[1])
    step("notify_search_archive")(lambda: call(admin, "DELETE", f"/channels/{ctx['sarch']}"))

    today = time.strftime("%Y-%m-%d", time.gmtime())
    tomorrow = time.strftime("%Y-%m-%d", time.gmtime(time.time() + 86400))
    yesterday = time.strftime("%Y-%m-%d", time.gmtime(time.time() - 86400))
    queries = [
        ("word", {"terms": "zebrafish"}),
        ("two_words", {"terms": "zebrafish aquarium"}),
        ("or", {"terms": "zebrafish aquarium", "is_or_search": True}),
        ("phrase", {"terms": "\"zebrafish genome\""}),
        ("phrase_quoted", {"terms": "\"quoted zebrafish phrase\""}),
        ("from", {"terms": "zebrafish from:bench0102"}),
        ("from_only", {"terms": "from:bench0101 in:notify-search"}),
        ("in", {"terms": "aquarium in:notify-search"}),
        ("in_display", {"terms": "aquarium in:\"Notify search\""}),
        ("excluded", {"terms": "zebrafish -genome"}),
        ("excluded_phrase", {"terms": "aquarium -\"zebra crossing\""}),
        ("hashtag", {"terms": "#maintenance"}),
        ("hashtag_word", {"terms": "#maintenance tank"}),
        ("mention", {"terms": "@bench0102"}),
        ("prefix", {"terms": "zebra*"}),
        ("prefix_short", {"terms": "aqua*"}),
        ("underscore", {"terms": "under_score"}),
        ("dash", {"terms": "dash-word"}),
        ("on", {"terms": f"zebrafish on:{today}"}),
        ("after", {"terms": f"zebrafish after:{yesterday}"}),
        ("before", {"terms": f"zebrafish before:{tomorrow}"}),
        ("before_past", {"terms": f"zebrafish before:{yesterday}"}),
        ("tz", {"terms": f"zebrafish on:{today}", "time_zone_offset": -36000}),
        ("tz_plus", {"terms": f"zebrafish after:{yesterday}", "time_zone_offset": 50400}),
        ("deleted_channels", {"terms": "zebrafish", "include_deleted_channels": True}),
        ("page0", {"terms": "zebrafish", "page": 0, "per_page": 2}),
        ("page1", {"terms": "zebrafish", "page": 1, "per_page": 2}),
        ("page_far", {"terms": "zebrafish", "page": 5, "per_page": 2}),
        ("empty", {"terms": ""}),
        ("nothing", {"terms": "nonexistentwordxyz"}),
        ("stopword", {"terms": "the"}),
        ("case", {"terms": "ZEBRAFISH"}),
        ("from_in_or", {"terms": "from:bench0101 from:bench0102 in:notify-search aquarium", "is_or_search": True}),
        ("bad_from", {"terms": "zebrafish from:nosuchuser"}),
        ("bad_in", {"terms": "zebrafish in:nosuchchannel"}),
        ("not_from", {"terms": "zebrafish -from:bench0102"}),
        ("not_in", {"terms": "zebrafish -in:notify-search"}),
        ("from_at", {"terms": "zebrafish from:@bench0102"}),
        ("in_tilde", {"terms": "zebrafish in:~notify-search"}),
        ("in_dm", {"terms": "secretword in:@bench0102"}),
        ("in_dm_name", {"terms": "secretword in:bench0101__bench0102"}),
        ("hashtag_excluded", {"terms": "tank -#maintenance"}),
        ("hashtag_prefix", {"terms": "#mainten*"}),
        ("hashtag_case", {"terms": "#Maintenance"}),
        ("phrase_prefix", {"terms": "\"zebrafish gen*\""}),
        ("excluded_only", {"terms": "-zebrafish"}),
        ("not_on", {"terms": f"zebrafish -on:{today}"}),
        ("bad_date", {"terms": "zebrafish on:2024-13-45"}),
        ("after_after", {"terms": f"zebrafish after:{yesterday} before:{tomorrow}"}),
        ("punct", {"terms": "zebrafish!!! (aquarium)"}),
        ("number", {"terms": "42"}),
        ("per_page_big", {"terms": "zebrafish", "per_page": 1000}),
        ("negative_page", {"terms": "zebrafish", "page": -1}),
    ]
    for name, q in queries:
        q = {"is_or_search": False, "include_deleted_channels": False, "time_zone_offset": 0, "page": 0,
             "per_page": 20} | q
        step(f"notify_search_{name}")(lambda q=q: body(call(u1, "POST", f"/teams/{team}/posts/search", q)))
    step("notify_search_all_teams")(lambda: body(call(u1, "POST", "/posts/search", {
        "terms": "zebrafish", "is_or_search": False, "time_zone_offset": 0, "page": 0, "per_page": 20})))
    step("notify_search_outsider")(lambda: body(call(s.u3, "POST", f"/teams/{team}/posts/search", {
        "terms": "zebrafish", "page": 0, "per_page": 20})))
    # recent mentions, as the web app asks: the mention keys, OR-ed
    step("notify_recent_mentions")(lambda: body(call(u2, "POST", f"/teams/{team}/posts/search", {
        "terms": "@bench0102 bench0102 ", "is_or_search": True, "include_deleted_channels": True,
        "time_zone_offset": 0, "page": 0, "per_page": 20})))
    step("notify_recent_mentions_all")(lambda: body(call(u2, "POST", "/posts/search", {
        "terms": "@bench0102 ", "is_or_search": True, "include_deleted_channels": True,
        "time_zone_offset": 0, "page": 0, "per_page": 20})))

    # ---- user search ----
    # bench0117 deactivated (Go 12.1 refuses to activate users past 250 on the bench: it stays so)
    def deactivate():
        ctx["inactive"] = call(admin, "GET", "/users/username/bench0117")[1]["id"]
        return call(admin, "PUT", f"/users/{ctx['inactive']}/active", {"active": False})
    step("notify_user_deactivate")(deactivate)
    user_queries = [
        ("plain", {"term": "bench010"}),
        ("team", {"term": "bench010", "team_id": team}),
        ("inactive", {"term": "bench0117", "allow_inactive": True}),
        ("inactive_not", {"term": "bench0117"}),
        ("inactive_prefix", {"term": "bench011", "allow_inactive": True, "team_id": team}),
        ("without_team", {"term": "bench", "without_team": True}),
        ("in_channel", {"term": "bench01", "in_channel_id": "@sch"}),
        ("not_in_channel", {"term": "bench010", "team_id": team, "not_in_channel_id": "@sch"}),
        ("limit", {"term": "bench0", "limit": 3}),
        ("email", {"term": "bench0105@"}),
    ]
    for name, q in user_queries:
        def us(q=q):
            q = {k: (ctx[v[1:]] if isinstance(v, str) and v.startswith("@") else v) for k, v in q.items()}
            return body(call(u1, "POST", "/users/search", q))
        step(f"notify_user_search_{name}")(us)
    step("notify_user_search_admin_inactive")(lambda: body(call(admin, "POST", "/users/search", {
        "term": "bench0117", "allow_inactive": True})))

    # ---- notifications ----
    def setup_mail():
        st, v, h = call(admin, "PUT", "/config/patch", {"EmailSettings": {
            "SMTPServer": "127.0.0.1", "SMTPPort": str(sink.port), "ConnectionSecurity": "",
            "EnableSMTPAuth": False, "EnableEmailBatching": False, "SendEmailNotifications": True,
            "SendPushNotifications": False}})
        return st, v, h
    step("notify_config")(setup_mail)

    def setup_users():
        for name in RECIPIENTS:
            users[name] = s.login(name)
            listeners[name] = Listener(s.base, users[name][0])
        listeners["u1"] = Listener(s.base, u1)
        time.sleep(0.5)
        r = keep("nch", call(admin, "POST", "/channels", {"team_id": team, "name": "notify-chan",
                                                          "display_name": "Notify chan", "type": "O"}))
        call(admin, "POST", f"/channels/{ctx['nch']}/members",
             {"user_ids": [u1_id] + [users[n][1] for n in RECIPIENTS]})
        return r
    step("notify_setup_users")(setup_users)

    def uid(n):
        return users[n][1]

    def tok(n):
        return users[n][0]

    statuses = {"bench0111": "offline", "bench0112": "away", "bench0113": "online", "bench0114": "dnd",
                "bench0115": "offline", "bench0116": "offline"}
    for n, stt in statuses.items():
        step(f"notify_status_{n}")(lambda n=n, stt=stt: call(admin, "PUT", f"/users/{uid(n)}/status", {
            "user_id": uid(n), "status": stt, **({"dnd_end_time": int(time.time()) + 7200} if stt == "dnd" else {})}))

    def email_off():
        me = call(tok("bench0115"), "GET", "/users/me")[1]
        props = dict(me["notify_props"], email="false")
        return call(tok("bench0115"), "PUT", f"/users/{uid('bench0115')}/patch", {"notify_props": props})
    step("notify_email_off")(email_off)
    step("notify_mute")(lambda: call(tok("bench0116"), "PUT", f"/channels/{ctx['nch']}/members/{uid('bench0116')}/notify_props",
                                     {"channel_id": ctx["nch"], "user_id": uid("bench0116"), "mark_unread": "mention"}))
    step("notify_drain")(lambda: (time.sleep(1), sink.take(), [lst.take() for lst in listeners.values()],
                                  (200, None, None))[-1])

    def seen():
        """What the last notifying step caused: mails, posted events per recipient, mention counts."""
        time.sleep(1.0)
        last, t0 = len(sink.mails), time.time()
        while time.time() - t0 < 2:
            time.sleep(0.3)
            if len(sink.mails) == last:
                break
            last = len(sink.mails)
        mails = sorted((mail_view(m) for m in sink.take()), key=lambda m: json.dumps(m, sort_keys=True))
        names = {users[n][1]: n for n in users} | {u1_id: "u1", u2_id: "u2"}
        events = {n: lst.take() for n, lst in listeners.items()}
        for evs in events.values():
            for e in evs:
                for k in ("mentions", "followers"):
                    if e.get(k):
                        e[k] = sorted(names.get(i, i) for i in json.loads(e[k]))
        st, members, _ = call(admin, "GET", f"/channels/{ctx['nch']}/members?page=0&per_page=100")
        counts = {names.get(m["user_id"], m["user_id"]): [m["msg_count"], m["mention_count"],
                                                          m.get("mention_count_root"), m.get("urgent_mention_count")]
                  for m in members or [] if m["user_id"] in names}
        return body((200, {"mails": mails, "events": events, "counts": counts}, None))

    def notify(name, fn):
        step(f"notify_{name}")(fn)
        step(f"notify_{name}_seen")(seen)

    def dm_to(n):
        def go():
            ch = call(u1, "POST", "/channels/direct", [u1_id, uid(n)])[1]["id"]
            return call(u1, "POST", "/posts", {"channel_id": ch, "message": f"direct to {n}"})
        return go

    notify("dm_offline", dm_to("bench0111"))
    notify("dm_online", dm_to("bench0113"))
    notify("dm_dnd", dm_to("bench0114"))
    notify("dm_email_off", dm_to("bench0115"))
    notify("mention_all", lambda: keep("mroot", call(u1, "POST", "/posts", {
        "channel_id": ctx["nch"], "message": " ".join("@" + n for n in RECIPIENTS) + " please look"})))
    notify("channel_mention", lambda: call(u1, "POST", "/posts", {"channel_id": ctx["nch"], "message": "@channel hello all"}))
    notify("here_mention", lambda: call(u1, "POST", "/posts", {"channel_id": ctx["nch"], "message": "@here who is around"}))
    notify("plain_post", lambda: call(u1, "POST", "/posts", {"channel_id": ctx["nch"], "message": "nobody in particular"}))
    # a thread bench0111 started: replies notify the followers
    notify("thread_root", lambda: keep("troot", call(tok("bench0111"), "POST", "/posts", {
        "channel_id": ctx["nch"], "message": "a thread of mine"})))
    notify("thread_reply", lambda: call(u1, "POST", "/posts", {"channel_id": ctx["nch"], "root_id": ctx["troot"],
                                                                "message": "a reply to the thread"}))
    notify("thread_reply2", lambda: call(tok("bench0112"), "POST", "/posts", {
        "channel_id": ctx["nch"], "root_id": ctx["troot"], "message": "another reply"}))
    notify("thread_reply_mention", lambda: call(u1, "POST", "/posts", {
        "channel_id": ctx["nch"], "root_id": ctx["troot"], "message": "@bench0116 @bench0113 in a thread"}))
    # channel-level email off for bench0112
    step("notify_channel_email_off")(lambda: call(tok("bench0112"), "PUT",
                                                  f"/channels/{ctx['nch']}/members/{uid('bench0112')}/notify_props",
                                                  {"channel_id": ctx["nch"], "user_id": uid("bench0112"), "email": "false"}))
    notify("mention_channel_email_off", lambda: call(u1, "POST", "/posts", {
        "channel_id": ctx["nch"], "message": "@bench0112 @bench0111 again"}))
    # bench0111 goes away, then online: away still mails, online does not
    step("notify_status_away")(lambda: call(admin, "PUT", f"/users/{uid('bench0111')}/status",
                                            {"user_id": uid("bench0111"), "status": "away"}))
    notify("mention_away", lambda: call(u1, "POST", "/posts", {"channel_id": ctx["nch"], "message": "@bench0111 away?"}))
    step("notify_status_online")(lambda: call(admin, "PUT", f"/users/{uid('bench0111')}/status",
                                              {"user_id": uid("bench0111"), "status": "online"}))
    notify("mention_online", lambda: call(u1, "POST", "/posts", {"channel_id": ctx["nch"], "message": "@bench0111 online?"}))
    # a group message with offline members
    notify("gm", lambda: call(u1, "POST", "/posts", {
        "channel_id": call(u1, "POST", "/channels/group", [u1_id, uid("bench0112"), uid("bench0116")])[1]["id"],
        "message": "group hello"}))
    # someone not in the channel: the sender is told (an ephemeral post)
    notify("mention_outsider", lambda: call(u1, "POST", "/posts", {"channel_id": ctx["nch"], "message": "@bench0120 come here"}))
    # custom mention keys and first name
    step("notify_status_offline_again")(lambda: call(admin, "PUT", f"/users/{uid('bench0111')}/status",
                                                     {"user_id": uid("bench0111"), "status": "offline"}))

    def keys():
        me = call(tok("bench0111"), "GET", "/users/me")[1]
        props = dict(me["notify_props"], mention_keys="aquaman,Zed", first_name="true")
        return call(tok("bench0111"), "PUT", f"/users/{uid('bench0111')}/patch", {"notify_props": props, "first_name": "Rufus"})
    step("notify_mention_keys")(keys)
    notify("mention_key", lambda: call(u1, "POST", "/posts", {"channel_id": ctx["nch"], "message": "hey aquaman, see this"}))
    notify("mention_first_name", lambda: call(u1, "POST", "/posts", {"channel_id": ctx["nch"], "message": "Rufus are you there"}))
    # channel mentions ignored by bench0111
    step("notify_ignore_channel_mentions")(lambda: call(tok("bench0111"), "PUT",
                                                        f"/channels/{ctx['nch']}/members/{uid('bench0111')}/notify_props",
                                                        {"channel_id": ctx["nch"], "user_id": uid("bench0111"),
                                                         "ignore_channel_mentions": "on"}))
    notify("all_mention_ignored", lambda: call(u1, "POST", "/posts", {"channel_id": ctx["nch"], "message": "@all listen"}))
    # an edit that adds a mention notifies nobody
    notify("edit_mention", lambda: call(u1, "PUT", f"/posts/{ctx['mroot']}/patch", {"message": "edited @bench0111 @bench0112"}))
    # user email off, channel email on: the channel wins for root posts, not for replies (CRT)
    step("notify_channel_email_on")(lambda: call(tok("bench0115"), "PUT",
                                                 f"/channels/{ctx['nch']}/members/{uid('bench0115')}/notify_props",
                                                 {"channel_id": ctx["nch"], "user_id": uid("bench0115"), "email": "true"}))
    notify("channel_email_wins", lambda: call(u1, "POST", "/posts", {"channel_id": ctx["nch"], "message": "@bench0115 root"}))
    notify("channel_email_reply", lambda: call(u1, "POST", "/posts", {"channel_id": ctx["nch"], "root_id": ctx["troot"],
                                                                       "message": "@bench0115 reply"}))
    # a DM reply in a thread, to an offline user
    notify("dm_thread", lambda: (lambda ch: call(u1, "POST", "/posts", {
        "channel_id": ch, "root_id": call(tok("bench0116"), "POST", "/posts", {"channel_id": ch, "message": "dm root"})[1]["id"],
        "message": "dm reply"}))(call(u1, "POST", "/channels/direct", [u1_id, uid("bench0116")])[1]["id"]))
    # mentions read back
    step("notify_mentions_search")(lambda: body(call(tok("bench0111"), "POST", f"/teams/{team}/posts/search", {
        "terms": "@bench0111 ", "is_or_search": True, "include_deleted_channels": True, "time_zone_offset": 0,
        "page": 0, "per_page": 20})))
    step("notify_threads")(lambda: body(call(tok("bench0111"), "GET",
                                             f"/users/{uid('bench0111')}/teams/{team}/threads?per_page=25&extended=false")))
    step("notify_unread")(lambda: body(call(tok("bench0111"), "GET", "/users/me/teams/unread"), ("",)))
