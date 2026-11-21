"""GitLab SSO end to end for sqldiff.py, against a fake GitLab run inside the scenario (one per server run, on a
free port): its authorize page redirects straight back with a code and the state, POST /oauth/token answers an
access token (or 400 when told to), GET /api/v4/user answers the GitLab user the step set. Both servers are
pointed at it with PUT /config/patch (and AllowedUntrustedInternalConnections, since it is on loopback).

The browser flow is driven with a cookie jar: /oauth/gitlab/login (or mobile_login, signup, or the follow_link of
/users/login/switch) -> the fake IdP -> /signup/gitlab/complete with code and state -> session cookies (or the
mobile page with the token) and the redirect. Each step answers a summary: status, where it sends the browser
(the /error page's message, the mobile page's link with the token blanked), the cookies it sets, and the rows it
leaves: the user's AuthService/AuthData/names, the oauth state tokens, the user's sessions.

Go 12.1 refuses to create users beyond 250, so new GitLab accounts are not made; existing bench users are switched
to GitLab instead: bench0140 through /users/login/switch (email -> GitLab and back), bench0141 by writing
AuthService/AuthData the way Go's UpdateAuthData does. Users bench0140..0145."""

import base64
import html
import http.client
import http.server
import json
import re
import threading
import urllib.parse

PW = "Bench-pass-123"
PW2 = "Bench-pass-456"


class FakeGitLab:
    """GitLab's three endpoints on 127.0.0.1, behaviour set by the step: user (the /api/v4/user answer),
    token_status (the token endpoint's status)."""

    def __init__(self):
        self.user = {}
        self.token_status = 200
        self.seen = []
        fake = self

        class H(http.server.BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def answer(self, status, body, headers=()):
                data = json.dumps(body).encode() if body is not None else b""
                self.send_response(status)
                for k, v in headers:
                    self.send_header(k, v)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def do_GET(self):
                u = urllib.parse.urlparse(self.path)
                q = dict(urllib.parse.parse_qsl(u.query))
                if u.path == "/oauth/authorize":
                    fake.seen.append(("authorize", sorted(q)))
                    to = q.get("redirect_uri", "") + "?" + urllib.parse.urlencode(
                        {"code": "code-" + q.get("client_id", ""), "state": q.get("state", "")})
                    return self.answer(302, None, [("Location", to)])
                if u.path == "/api/v4/user":
                    fake.seen.append(("user", self.headers.get("Authorization")))
                    return self.answer(200, fake.user)
                self.answer(404, {"message": "404 Not Found"})

            def do_POST(self):
                n = int(self.headers.get("Content-Length") or 0)
                form = dict(urllib.parse.parse_qsl(self.rfile.read(n).decode()))
                if urllib.parse.urlparse(self.path).path == "/oauth/token":
                    fake.seen.append(("token", sorted(form), form.get("grant_type"), form.get("code")))
                    if fake.token_status != 200:
                        return self.answer(fake.token_status, {"error": "invalid_grant",
                                                               "error_description": "The provided code is invalid."})
                    return self.answer(200, {"access_token": "gl-access-token", "token_type": "Bearer",
                                             "expires_in": 7200, "refresh_token": "gl-refresh", "scope": "read_user",
                                             "created_at": 1700000000})
                self.answer(404, {"message": "404 Not Found"})

        self.srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
        self.port = self.srv.server_address[1]
        threading.Thread(target=self.srv.serve_forever, daemon=True).start()

    def url(self, path):
        return f"http://127.0.0.1:{self.port}{path}"


def gl_user(n, **over):
    return {"id": 9000 + n, "username": f"bench0{n}", "email": f"bench0{n}@example.com", "name": f"Bench User{n}",
            "state": "active", **over}


def register(s):
    step, call, ctx, body, psql = s.step, s.call, s.ctx, s.body, s.psql
    admin = s.admin
    base = urllib.parse.urlparse(s.base)
    idp = FakeGitLab()

    def raw(method, path, headers=None, data=None, where=None):
        """A request (to the server unless where = (host, port)), redirects not followed."""
        host, port = where or (base.hostname, base.port)
        c = http.client.HTTPConnection(host, port, timeout=30)
        c.request(method, path, body=data, headers=headers or {})
        r = c.getresponse()
        out = r.read()
        c.close()
        return r.status, out, r.headers

    def cookies_of(h):
        """Set-Cookie as name -> value ('' for a cleared one)."""
        out = {}
        for c in h.get_all("Set-Cookie") or []:
            k, _, rest = c.partition("=")
            v = rest.split(";")[0]
            out[k] = "" if "max-age=0" in c.lower() else v
        return out

    def jar_header(jar):
        return "; ".join(f"{k}={v}" for k, v in jar.items() if v)

    def uid(name):
        return psql(f"SELECT Id FROM Users WHERE Username = '{name}'").strip()

    def user_row(email):
        return psql("SELECT Username, AuthService, COALESCE(AuthData, ''), Password = '', Email, FirstName, LastName, "
                    "EmailVerified, MfaActive, DeleteAt <> 0, LastLogin > 0, NotifyProps::jsonb ->> 'mention_keys' "
                    f"FROM Users WHERE Email = '{email}' OR Username = '{email.split('@')[0]}'").strip().splitlines()

    def sessions(name):
        return psql("SELECT Props::jsonb ->> 'isOAuthUser', Props::jsonb ->> 'isMobile', Props::jsonb ->> 'isSaml', "
                    "round((ExpiresAt - CreateAt) / 3600000.0), IsOAuth, DeviceId FROM Sessions "
                    f"WHERE UserId = '{uid(name)}' ORDER BY CreateAt").strip().splitlines()

    def oauth_tokens():
        return psql("SELECT count(*) FROM Tokens WHERE Type = 'oauth'").strip()

    def where_to(st, out, h):
        """Where an answer sends the browser: a Location, or the /error page an HTML answer loads, or the link
        of the mobile pages; tokens and signatures blanked."""
        loc = h.get("Location")
        page = out.decode("utf-8", "replace")
        if not loc:
            m = re.search(r'url=([^"]+)"', page)
            loc = html.unescape(m.group(1)) if m else None
        if not loc:
            return None
        u = urllib.parse.urlparse(loc)
        q = urllib.parse.parse_qsl(u.query, keep_blank_values=True)
        blank = {"s", "MMAUTHTOKEN", "MMCSRF", "state", "server_token"}
        q = {k: ("<set>" if k in blank and v else v) for k, v in q}
        netloc = u.netloc.replace(f"127.0.0.1:{idp.port}", "<idp>").replace(base.netloc, "<server>")
        return {"to": f"{u.scheme}://{netloc}{u.path}" if u.netloc else u.path, "query": q}

    def flow(start, name, email, jar=None, state_fix=None, keep_cookie=True, headers=None):
        """start (a server path, or ('follow', url) for an IdP link already in hand) -> IdP -> complete.
        Returns the step's answer: a summary of each hop and the rows after."""
        jar = dict(jar or {})
        hops = []
        if isinstance(start, tuple):
            loc = start[1]
        else:
            st, out, h = raw("GET", start, {"Cookie": jar_header(jar), **(headers or {})})
            jar.update(cookies_of(h))
            loc = h.get("Location") or ""
            hops.append({"status": st, "where": where_to(st, out, h), "cookies": sorted(cookies_of(h))})
            if st != 302 or not loc.startswith(idp.url("")):
                return st, {"hops": hops}, h
        u = urllib.parse.urlparse(loc)
        st, out, h = raw("GET", u.path + "?" + u.query, where=("127.0.0.1", idp.port))
        back = urllib.parse.urlparse(h.get("Location") or "")
        hops.append({"idp": st, "redirect_uri": f"{back.scheme}://{back.netloc}{back.path}",
                     "auth_query": sorted(urllib.parse.parse_qs(u.query))})
        q = dict(urllib.parse.parse_qsl(back.query))
        if state_fix:
            q["state"] = state_fix(q.get("state", ""))
        if not keep_cookie:
            jar.pop("MMOAUTH", None)
        st, out, h = raw("GET", back.path + "?" + urllib.parse.urlencode(q),
                         {"Cookie": jar_header(jar), **(headers or {})})
        set_c = cookies_of(h)
        jar.update(set_c)
        ctx["jar_" + name] = jar
        page = out.decode("utf-8", "replace")
        m = re.search(r"MMAUTHTOKEN=([a-z0-9]{26})", page)
        if m:
            ctx["mobile_token_" + name] = m.group(1)
        hops.append({"status": st, "ctype": h.get("Content-Type"), "where": where_to(st, out, h),
                     "cookies": sorted(set_c), "cleared": sorted(k for k, v in set_c.items() if not v),
                     "token_header": bool(h.get("Token")),
                     "mobile_page": "mobile app message" in page, "web_error": "web error message" in page})
        return st, {"hops": hops, "user": user_row(email), "sessions": sessions(name), "oauth_tokens": oauth_tokens()}, h

    # ---- config ----
    def config():
        return call(admin, "PUT", "/config/patch", {
            "ServiceSettings": {"AllowedUntrustedInternalConnections": "127.0.0.1"},
            "GitLabSettings": {"Enable": True, "Id": "gl-client", "Secret": "gl-secret", "Scope": "",
                               "AuthEndpoint": idp.url("/oauth/authorize"),
                               "TokenEndpoint": idp.url("/oauth/token"),
                               "UserAPIEndpoint": idp.url("/api/v4/user")}})[0], None, None
    step("sso_config")(config)
    step("sso_client_config")(lambda: (lambda r: body((r[0], {k: r[1].get(k) for k in (
        "EnableSignUpWithGitLab", "GitLabButtonText", "GitLabButtonColor")}, r[2])))(
        call(None, "GET", "/config/client?format=old")))

    # ---- email -> GitLab for bench0140, through /users/login/switch ----
    def switch_to_gitlab(password=PW):
        st, v, h = call(None, "POST", "/users/login/switch", {
            "current_service": "email", "new_service": "gitlab", "email": "bench0140@example.com",
            "password": password})
        jar = cookies_of(h)
        if st == 200:
            ctx["switch_link"], ctx["switch_jar"] = v["follow_link"], jar
            v = {"follow_link": where_to(302, b"", _Hdr(v["follow_link"]))}
        return body((st, {"answer": v, "cookies": sorted(jar), "oauth_tokens": oauth_tokens()}, h))
    step("sso_switch_wrong_password")(lambda: switch_to_gitlab("Wrong-pass-1"))
    step("sso_switch_login_first")(lambda: (lambda t: body((200, {"sessions": sessions("bench0140")}, None)))(
        s.login("bench0140")))
    step("sso_switch_email_to_gitlab")(switch_to_gitlab)

    def switch_complete():
        idp.user = gl_user(140)
        return body(flow(("follow", ctx.get("switch_link", "")), "bench0140", "bench0140@example.com",
                         jar=ctx.get("switch_jar")))
    step("sso_switch_complete")(switch_complete)
    step("sso_switch_password_login")(lambda: body(call(None, "POST", "/users/login",
                                                        {"login_id": "bench0140", "password": PW})))
    step("sso_login_type")(lambda: body(call(None, "POST", "/users/login/type", {"login_id": "bench0140"})))

    # ---- web login of the switched user ----
    def web_login(n, path="/oauth/gitlab/login?redirect_to=%2Fbench%2Fchannels%2Fbench-public", **user):
        idp.user = gl_user(n, **user)
        return body(flow(path, f"bench0{n}", f"bench0{n}@example.com"))
    step("sso_login_web")(lambda: web_login(140))
    step("sso_login_web_me")(lambda: body(call(ctx.get("jar_bench0140", {}).get("MMAUTHTOKEN"), "GET", "/users/me")))
    step("sso_login_web_again")(lambda: web_login(140, "/oauth/gitlab/login"))
    step("sso_login_web_bad_redirect")(lambda: web_login(140, "/oauth/gitlab/login?redirect_to=https%3A%2F%2Fevil.example.com%2F"))

    # ---- mobile ----
    step("sso_mobile_login")(lambda: web_login(140, "/oauth/gitlab/mobile_login?redirect_to=mmauth%3A%2F%2Fcallback"))
    step("sso_mobile_token_use")(lambda: body(call(ctx.get("mobile_token_bench0140"), "GET", "/users/me")))
    step("sso_mobile_login_old")(lambda: web_login(140, "/oauth/gitlab/mobile_login"))
    step("sso_mobile_bad_scheme")(lambda: web_login(140, "/oauth/gitlab/mobile_login?redirect_to=evil%3A%2F%2Fcallback"))

    # ---- bench0141 switched in the database (UpdateAuthData), then GitLab renames it ----
    def db_switch():
        psql("UPDATE Users SET Password = '', AuthService = 'gitlab', AuthData = '9141', FailedAttempts = 0, "
             "MfaActive = false, MfaSecret = '', UpdateAt = (extract(epoch from now()) * 1000)::bigint "
             "WHERE Username = 'bench0141'")
        return 200, None, None
    step("sso_db_switch")(db_switch)
    step("sso_login_attrs_changed")(lambda: web_login(141, name="Renamed Person Here", username="bench0141x"))
    step("sso_login_attrs_taken")(lambda: web_login(141, username="bench0142", email="bench0142@example.com"))

    # ---- failures ----
    def bad_state(fix, keep_cookie=True):
        idp.user = gl_user(140)
        return body(flow("/oauth/gitlab/login", "bench0140", "bench0140@example.com", state_fix=fix,
                         keep_cookie=keep_cookie))

    def other_token(st):
        p = json.loads(base64.b64decode(st))
        p["token"] = "x" * 64
        return base64.b64encode(json.dumps(p).encode()).decode()

    def other_action(st):
        p = json.loads(base64.b64decode(st))
        p["action"] = "signup"
        return base64.b64encode(json.dumps(p).encode()).decode()
    step("sso_bad_state_garbage")(lambda: bad_state(lambda st: "not base64!"))
    step("sso_bad_state_token")(lambda: bad_state(other_token))
    step("sso_bad_state_action")(lambda: bad_state(other_action))
    step("sso_bad_state_no_cookie")(lambda: bad_state(None, keep_cookie=False))
    step("sso_missing_code")(lambda: (lambda r: body((r[0], {"where": where_to(*r)}, r[2])))(
        raw("GET", "/signup/gitlab/complete?state=abc")))
    step("sso_access_denied")(lambda: (lambda r: body((r[0], {"where": where_to(*r)}, r[2])))(
        raw("GET", "/signup/gitlab/complete?error=access_denied")))

    def token_fails():
        idp.token_status = 400
        try:
            return web_login(140)
        finally:
            idp.token_status = 200
    step("sso_token_exchange_fails")(token_fails)

    def token_fails_mobile():
        idp.token_status = 400
        try:
            return web_login(140, "/oauth/gitlab/mobile_login?redirect_to=mmauth%3A%2F%2Fcallback")
        finally:
            idp.token_status = 200
    step("sso_token_exchange_fails_mobile")(token_fails_mobile)
    step("sso_user_no_email")(lambda: web_login(140, email=""))

    # ---- signup ----
    step("sso_restrict_domains")(lambda: (call(admin, "PUT", "/config/patch", {
        "TeamSettings": {"RestrictCreationToDomains": "example.com"}})[0], None, None))
    step("sso_signup_other_domain")(lambda: (setattr(idp, "user", {
        "id": 9999, "username": "outsider", "email": "outsider@other.org", "name": "Out Sider"}),
        body(flow("/oauth/gitlab/signup", "outsider", "outsider@other.org")))[1])
    step("sso_login_other_domain")(lambda: (setattr(idp, "user", {
        "id": 9998, "username": "outsider2", "email": "outsider2@other.org", "name": "Out Sider"}),
        body(flow("/oauth/gitlab/login", "outsider2", "outsider2@other.org")))[1])
    step("sso_signup_email_taken")(lambda: (setattr(idp, "user", gl_user(143)),
                                            body(flow("/oauth/gitlab/signup", "bench0143", "bench0143@example.com")))[1])
    step("sso_signup_existing_gitlab")(lambda: (setattr(idp, "user", gl_user(140)),
                                                body(flow("/oauth/gitlab/signup", "bench0140", "bench0140@example.com")))[1])
    step("sso_unrestrict_domains")(lambda: (call(admin, "PUT", "/config/patch", {
        "TeamSettings": {"RestrictCreationToDomains": ""}})[0], None, None))

    # ---- new accounts: below Go's 250 active users (bench0240.. deactivated in this run's database) ----
    def below_cap():
        psql("UPDATE Users SET DeleteAt = 1 WHERE Username >= 'bench0240' AND Username <= 'bench0499'")
        return 200, None, None
    step("sso_below_cap")(below_cap)
    step("sso_restrict_domains_2")(lambda: (call(admin, "PUT", "/config/patch", {
        "TeamSettings": {"RestrictCreationToDomains": "example.com"}})[0], None, None))
    step("sso_signup_other_domain_2")(lambda: (setattr(idp, "user", {
        "id": 9999, "username": "outsider", "email": "outsider@other.org", "name": "Out Sider"}),
        body(flow("/oauth/gitlab/signup", "outsider", "outsider@other.org")))[1])
    step("sso_unrestrict_domains_2")(lambda: (call(admin, "PUT", "/config/patch", {
        "TeamSettings": {"RestrictCreationToDomains": ""}})[0], None, None))
    step("sso_signup_new")(lambda: (setattr(idp, "user", {
        "id": 9990, "username": "Gl.Newcomer", "email": "GL.Newcomer@Example.com", "name": "New Comer Person"}),
        body(flow("/oauth/gitlab/signup", "gl.newcomer", "gl.newcomer@example.com")))[1])
    step("sso_login_new_again")(lambda: (setattr(idp, "user", {
        "id": 9990, "username": "gl.newcomer", "email": "gl.newcomer@example.com", "name": "New Comer"}),
        body(flow("/oauth/gitlab/login", "gl.newcomer", "gl.newcomer@example.com")))[1])
    step("sso_login_creates_taken_name")(lambda: (setattr(idp, "user", {
        "id": 9991, "username": "bench0144", "email": "someone.else@example.com", "name": "Some One"}),
        body(flow("/oauth/gitlab/login", "bench01440", "someone.else@example.com")))[1])
    step("sso_mobile_creates")(lambda: (setattr(idp, "user", {
        "id": 9992, "username": "mobile.new", "email": "mobile.new@example.com", "name": "Mo"}),
        body(flow("/oauth/gitlab/mobile_login?redirect_to=mmauth%3A%2F%2Fcallback", "mobile.new",
                  "mobile.new@example.com")))[1])
    step("sso_new_user_me")(lambda: body(call(ctx.get("jar_gl.newcomer", {}).get("MMAUTHTOKEN"), "GET", "/users/me")))
    step("sso_new_user_teams")(lambda: body(call(ctx.get("jar_gl.newcomer", {}).get("MMAUTHTOKEN"), "GET",
                                                 "/users/me/teams")))

    # ---- GitLab -> email for bench0140 ----
    def switch_back(token, email="bench0140@example.com"):
        st, v, h = call(token, "POST", "/users/login/switch", {
            "current_service": "gitlab", "new_service": "email", "email": email, "new_password": PW2})
        return body((st, {"answer": v, "user": user_row("bench0140@example.com"), "sessions": sessions("bench0140")}, h))
    step("sso_switch_back_anon")(lambda: switch_back(None))
    step("sso_switch_back_other_user")(lambda: switch_back(ctx.get("jar_bench0141", {}).get("MMAUTHTOKEN")))
    step("sso_switch_back")(lambda: switch_back(ctx.get("jar_bench0140", {}).get("MMAUTHTOKEN")))
    step("sso_switch_back_login")(lambda: body(call(None, "POST", "/users/login", {"login_id": "bench0140",
                                                                                   "password": PW2})))
    step("sso_after_switch_back_gitlab")(lambda: web_login(140))
    step("sso_switch_back_not_oauth")(lambda: (lambda t: switch_back(t))(s.login("bench0140", PW2)[0]))

    # ---- GitLab off ----
    step("sso_disable")(lambda: (call(admin, "PUT", "/config/patch", {"GitLabSettings": {"Enable": False}})[0],
                                 None, None))
    step("sso_login_disabled")(lambda: web_login(141))


class _Hdr(dict):
    def __init__(self, loc):
        super().__init__(Location=loc)
