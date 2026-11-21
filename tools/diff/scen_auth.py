"""Authentication for sqldiff.py: logins (username, email, wrong password and the lockout after MaximumLoginAttempts,
a deactivated user, a device id and the session it makes), logout, revoking sessions, attaching a device id, MFA
(secret, activation with a TOTP, login with the token, deactivation), password reset and email verification through
the tokens they write, the OAuth 2.0 provider (an app, authorize, code exchange, the token in use, refresh,
deauthorize, delete), GitLab SSO up to the redirect, and the CSRF rules for cookie sessions.

The users are bench0120.. so that no other module's users are touched. Answers are compared by value; where the
interesting part is a header (cookies, Location) or a row, the step answers a summary of it instead."""

import base64
import hashlib
import hmac
import http.client
import json
import struct
import time
import urllib.parse

PW = "Bench-pass-123"
PW2 = "Bench-pass-789"
CB = "http://localhost:9/cb"


def totp(secret, at=None):
    """RFC 6238: HMAC-SHA1, 30 s steps, 6 digits, as Google Authenticator and Mattermost's mfa package."""
    key = base64.b32decode(secret.upper() + "=" * (-len(secret) % 8))
    msg = struct.pack(">Q", int((time.time() if at is None else at) // 30))
    d = hmac.new(key, msg, hashlib.sha1).digest()
    o = d[-1] & 15
    return "%06d" % ((struct.unpack(">I", d[o:o + 4])[0] & 0x7FFFFFFF) % 1000000)


def register(s):
    step, call, ctx, body, psql = s.step, s.call, s.ctx, s.body, s.psql
    admin = s.admin
    url = urllib.parse.urlparse(s.base)

    def raw(method, path, headers=None, data=None):
        """A request at the server's root, redirects not followed: (status, parsed or None, headers)."""
        c = http.client.HTTPConnection(url.hostname, url.port, timeout=30)
        c.request(method, path, body=data, headers=headers or {})
        r = c.getresponse()
        out = r.read()
        try:
            val = json.loads(out) if out[:1] in (b"{", b"[") else None
        except ValueError:
            val = None
        c.close()
        return r.status, val, r.headers

    def uid(name):
        return psql(f"SELECT Id FROM Users WHERE Username = '{name}'").strip()

    def row(name, cols="FailedAttempts, MfaActive, MfaSecret <> '', EmailVerified, DeleteAt <> 0"):
        return psql(f"SELECT {cols} FROM Users WHERE Username = '{name}'").strip()

    def login(login_id, password=PW, **extra):
        return call(None, "POST", "/users/login", {"login_id": login_id, "password": password, **extra})

    def with_row(r, name, cols=None):
        """The answer plus the user's row after it."""
        st, v, h = r
        return st, {"answer": v, "row": row(name) if cols is None else row(name, cols)}, h

    def tok_of(r, k):
        st, v, h = r
        if st == 200:
            ctx[k] = h["Token"]
        return r

    def sessions_of(name):
        """The user's sessions as rows: device id, props of interest, expiry set or not."""
        return psql("SELECT DeviceId, Props::jsonb ->> 'platform', ExpiresAt > 0, IsOAuth FROM Sessions "
                    f"WHERE UserId = '{uid(name)}' ORDER BY CreateAt").strip().splitlines()

    def sess(r, name):
        st, v, h = r
        return st, {"answer": v, "sessions": sessions_of(name)}, h

    # ---- config ----
    step("auth_config")(lambda: call(admin, "PUT", "/config/patch", {
        "ServiceSettings": {"MaximumLoginAttempts": 3, "EnableMultifactorAuthentication": True,
                            "EnableOAuthServiceProvider": True, "EnableUserAccessTokens": True},
        "EmailSettings": {"RequireEmailVerification": False},
        "GitLabSettings": {"Enable": True, "Id": "gitlab-client-id", "Secret": "gitlab-secret",
                           "AuthEndpoint": "https://gitlab.example.com/oauth/authorize",
                           "TokenEndpoint": "https://gitlab.example.com/oauth/token",
                           "UserAPIEndpoint": "https://gitlab.example.com/api/v4/user"}}))

    # ---- logins ----
    step("auth_login_username")(lambda: body(tok_of(login("bench0120"), "t120")))
    step("auth_login_email")(lambda: body(login("bench0120@example.com")))
    step("auth_login_email_case")(lambda: body(login("BENCH0120")))
    step("auth_login_no_user")(lambda: body(login("nobody-at-all")))
    step("auth_login_empty")(lambda: body(call(None, "POST", "/users/login", {"login_id": "bench0120"})))
    step("auth_login_type")(lambda: body(call(None, "POST", "/users/login/type", {"login_id": "bench0120"})))
    for i in range(1, 4):
        step(f"auth_login_wrong_{i}")(lambda: body(with_row(login("bench0121", "Wrong-pass-1"), "bench0121")))
    step("auth_login_locked")(lambda: body(with_row(login("bench0121"), "bench0121")))
    step("auth_login_locked_wrong")(lambda: body(with_row(login("bench0121", "Wrong-pass-1"), "bench0121")))
    step("auth_unlock_admin_pw")(lambda: body(with_row(call(admin, "PUT", f"/users/{uid('bench0121')}/password",
                                                            {"new_password": PW}), "bench0121")))
    step("auth_login_unlocked")(lambda: body(with_row(login("bench0121"), "bench0121")))
    step("auth_login_wrong_then_ok")(lambda: (login("bench0122", "Wrong-pass-1"),
                                              body(with_row(login("bench0122"), "bench0122")))[1])
    step("auth_deactivate")(lambda: call(admin, "DELETE", f"/users/{uid('bench0123')}"))
    step("auth_login_deactivated")(lambda: body(with_row(login("bench0123"), "bench0123")))
    step("auth_login_deactivated_wrong")(lambda: body(with_row(login("bench0123", "Wrong-pass-1"), "bench0123")))

    # ---- device ids and sessions ----
    step("auth_login_device")(lambda: body(sess(tok_of(login("bench0124", device_id="android_rn-v2:dev-one"), "d1"),
                                                "bench0124")))
    step("auth_login_device_again")(lambda: body(sess(tok_of(login("bench0124", device_id="android_rn-v2:dev-one"), "d2"),
                                                      "bench0124")))
    step("auth_old_device_session")(lambda: body(call(ctx.get("d1"), "GET", "/users/me")))
    step("auth_login_plain")(lambda: body(sess(tok_of(login("bench0124"), "d3"), "bench0124")))
    step("auth_attach_device")(lambda: body(sess(call(ctx.get("d3"), "PUT", "/users/sessions/device",
                                                      {"device_id": "apple_rn-v2:dev-two"}), "bench0124")))
    step("auth_attach_same_device")(lambda: body(sess(call(ctx.get("d2"), "PUT", "/users/sessions/device",
                                                           {"device_id": "apple_rn-v2:dev-two"}), "bench0124")))
    step("auth_attach_device_empty")(lambda: body(call(ctx.get("d2"), "PUT", "/users/sessions/device", {})))
    step("auth_d3_after_attach")(lambda: body(call(ctx.get("d3"), "GET", "/users/me")))
    step("auth_sessions_list")(lambda: body(call(ctx.get("d2"), "GET", f"/users/{uid('bench0124')}/sessions"),
                                            unordered=("",)))
    step("auth_logout")(lambda: body(sess(call(ctx.get("d2"), "POST", "/users/logout"), "bench0124")))
    step("auth_after_logout")(lambda: body(call(ctx.get("d2"), "GET", "/users/me")))
    step("auth_logout_again")(lambda: body(call(ctx.get("d2"), "POST", "/users/logout")))
    step("auth_logout_anon")(lambda: body(call(None, "POST", "/users/logout")))

    # revoke one, revoke all for a user
    def two_logins():
        tok_of(login("bench0125"), "r1")
        return body(sess(tok_of(login("bench0125"), "r2"), "bench0125"))
    step("auth_revoke_setup")(two_logins)

    def revoke_one():
        sid = psql(f"SELECT Id FROM Sessions WHERE Token = '{ctx.get('r1')}'").strip()
        return body(sess(call(ctx.get("r2"), "POST", f"/users/{uid('bench0125')}/sessions/revoke",
                              {"session_id": sid}), "bench0125"))
    step("auth_revoke_one")(revoke_one)
    step("auth_revoke_one_used")(lambda: body(call(ctx.get("r1"), "GET", "/users/me")))
    step("auth_revoke_bad_id")(lambda: body(call(ctx.get("r2"), "POST", f"/users/{uid('bench0125')}/sessions/revoke",
                                                 {"session_id": "x" * 26})))
    step("auth_revoke_other_user")(lambda: body(call(ctx.get("r2"), "POST", f"/users/{uid('bench0120')}/sessions/revoke/all")))
    step("auth_revoke_all")(lambda: body(sess(call(admin, "POST", f"/users/{uid('bench0125')}/sessions/revoke/all"),
                                              "bench0125")))
    step("auth_revoke_all_used")(lambda: body(call(ctx.get("r2"), "GET", "/users/me")))

    # ---- MFA ----
    step("auth_mfa_login")(lambda: tok_of(login("bench0126"), "m1"))

    def mfa_generate():
        st, v, h = call(ctx.get("m1"), "POST", f"/users/{uid('bench0126')}/mfa/generate")
        if st == 200:
            ctx["mfa"] = v["secret"]
            v = {"secret_len": len(v["secret"]), "qr_code_png": base64.b64decode(v["qr_code"])[:8].hex()}
        return body(with_row((st, v, h), "bench0126"))
    step("auth_mfa_generate")(mfa_generate)
    step("auth_mfa_activate_bad")(lambda: body(with_row(call(ctx.get("m1"), "PUT", f"/users/{uid('bench0126')}/mfa",
                                                             {"activate": True, "code": "000000"}), "bench0126")))
    step("auth_mfa_activate")(lambda: body(with_row(call(ctx.get("m1"), "PUT", f"/users/{uid('bench0126')}/mfa",
                                                         {"activate": True, "code": totp(ctx.get("mfa", "A" * 32))}),
                                                    "bench0126")))
    step("auth_mfa_login_type")(lambda: body(call(None, "POST", "/users/login/type", {"login_id": "bench0126"})))
    step("auth_mfa_login_no_token")(lambda: body(with_row(login("bench0126"), "bench0126")))
    step("auth_mfa_login_bad_token")(lambda: body(with_row(login("bench0126", token="123456"), "bench0126")))
    step("auth_mfa_login_wrong_pw_token")(lambda: body(with_row(
        login("bench0126", "Wrong-pass-1", token=totp(ctx.get("mfa", "A" * 32))), "bench0126")))

    def mfa_login_fresh():
        """A code of a time step not used yet: the activation (or the step before) used the current one."""
        time.sleep(30 - time.time() % 30 + 1)
        ctx["code"] = totp(ctx.get("mfa", "A" * 32))
        return body(with_row(tok_of(login("bench0126", token=ctx["code"]), "m2"), "bench0126"))
    step("auth_mfa_login_token")(mfa_login_fresh)
    step("auth_mfa_login_reused")(lambda: body(with_row(login("bench0126", token=ctx.get("code", "000000")), "bench0126")))
    step("auth_mfa_deactivate")(lambda: body(with_row(call(ctx.get("m2"), "PUT", f"/users/{uid('bench0126')}/mfa",
                                                           {"activate": False}), "bench0126")))
    step("auth_mfa_login_after")(lambda: body(with_row(login("bench0126"), "bench0126")))

    # ---- password reset ----
    def token_rows(kind):
        return psql(f"SELECT Type, Extra::jsonb ->> 'UserId' = '{uid('bench0127')}', Extra::jsonb ? 'Email', "
                    f"length(Token) FROM Tokens WHERE Type = '{kind}'").strip().splitlines()

    def reset_send(email):
        st, v, h = call(None, "POST", "/users/password/reset/send", {"email": email})
        return body((st, {"answer": v, "tokens": token_rows("password_recovery")}, h))
    step("auth_reset_send")(lambda: reset_send("bench0127@example.com"))
    step("auth_reset_send_unknown")(lambda: reset_send("nobody@example.com"))
    step("auth_reset_bad_token")(lambda: body(call(None, "POST", "/users/password/reset",
                                                   {"token": "x" * 64, "new_password": PW2})))

    def reset():
        t = psql("SELECT Token FROM Tokens WHERE Type = 'password_recovery' ORDER BY CreateAt DESC LIMIT 1").strip()
        ctx["reset"] = t
        st, v, h = call(None, "POST", "/users/password/reset", {"token": t, "new_password": PW2})
        return body((st, {"answer": v, "tokens": token_rows("password_recovery")}, h))
    step("auth_reset")(reset)
    step("auth_reset_reuse")(lambda: body(call(None, "POST", "/users/password/reset",
                                               {"token": ctx.get("reset", "x"), "new_password": PW})))
    step("auth_reset_login_old")(lambda: body(login("bench0127")))
    step("auth_reset_login_new")(lambda: body(login("bench0127", PW2)))

    # ---- email verification ----
    # The bench users' emails are not verified.

    def verify_send(email):
        st, v, h = call(None, "POST", "/users/email/verify/send", {"email": email})
        return body((st, {"answer": v, "tokens": token_rows("verify_email")}, h))
    step("auth_verify_send")(lambda: verify_send("bench0127@example.com"))
    step("auth_verify_send_unknown")(lambda: verify_send("nobody@example.com"))
    step("auth_verify_bad")(lambda: body(call(None, "POST", "/users/email/verify", {"token": "y" * 64})))

    def verify():
        t = psql("SELECT Token FROM Tokens WHERE Type = 'verify_email' ORDER BY CreateAt DESC LIMIT 1").strip()
        st, v, h = call(None, "POST", "/users/email/verify", {"token": t})
        return body(with_row((st, {"answer": v, "tokens": token_rows("verify_email")}, h), "bench0127"))
    step("auth_verify")(verify)
    step("auth_verify_member")(lambda: body(with_row(call(admin, "POST", f"/users/{uid('bench0128')}/email/verify/member"),
                                                     "bench0128")))

    # ---- OAuth 2.0 provider ----
    def app_create():
        st, v, h = s.keep("app", call(admin, "POST", "/oauth/apps", {
            "name": "sqldiff app", "description": "an app", "homepage": "http://localhost:9/",
            "callback_urls": [CB], "is_trusted": False}))
        if st == 201:
            ctx["app_secret"] = v["client_secret"]
        return body((st, v, h))
    step("auth_oauth_app")(app_create)
    step("auth_oauth_login")(lambda: tok_of(login("bench0129"), "o1"))

    def authorize(state="st4te", **over):
        req = {"response_type": "code", "client_id": ctx.get("app", "x"), "redirect_uri": CB, "state": state,
               "scope": "", **over}
        st, v, h = raw("POST", "/oauth/authorize", {"Authorization": "Bearer " + str(ctx.get("o1")),
                                                     "Content-Type": "application/json"}, json.dumps(req))
        if st == 200 and v and "redirect" in v:
            q = urllib.parse.parse_qs(urllib.parse.urlparse(v["redirect"]).query)
            ctx["code"] = q.get("code", [""])[0]
            v = {"redirect": v["redirect"].split("?")[0], "query": sorted(q)}
        return body((st, v, h))
    step("auth_oauth_authorize")(authorize)
    step("auth_oauth_authorize_bad_redirect")(lambda: authorize(redirect_uri="http://evil.example.com/cb"))
    step("auth_oauth_authorize_page")(lambda: (lambda r: body((r[0], {"location": urllib.parse.urlparse(
        r[2].get("Location") or "").path}, r[2])))(raw("GET", "/oauth/authorize?" + urllib.parse.urlencode(
            {"response_type": "code", "client_id": ctx.get("app", "x"), "redirect_uri": CB, "state": "s"}),
            {"Authorization": "Bearer " + str(ctx.get("o1"))})))
    step("auth_oauth_authorized_apps")(lambda: body(call(ctx.get("o1"), "GET",
                                                         f"/users/{uid('bench0129')}/oauth/apps/authorized")))

    def exchange(form, keep=True):
        st, v, h = raw("POST", "/oauth/access_token", {"Content-Type": "application/x-www-form-urlencoded"},
                       urllib.parse.urlencode(form))
        if st == 200 and keep:
            ctx["access"], ctx["refresh"] = v["access_token"], v["refresh_token"]
        return body((st, v, h))
    step("auth_oauth_token_bad_secret")(lambda: exchange({
        "grant_type": "authorization_code", "client_id": ctx.get("app", "x"), "client_secret": "nope",
        "code": ctx.get("code", "x"), "redirect_uri": CB}))
    step("auth_oauth_token")(lambda: exchange({
        "grant_type": "authorization_code", "client_id": ctx.get("app", "x"), "client_secret": ctx.get("app_secret"),
        "code": ctx.get("code", "x"), "redirect_uri": CB}))
    step("auth_oauth_token_reuse")(lambda: exchange({
        "grant_type": "authorization_code", "client_id": ctx.get("app", "x"), "client_secret": ctx.get("app_secret"),
        "code": ctx.get("code", "x"), "redirect_uri": CB}, keep=False))
    step("auth_oauth_use")(lambda: body(call(ctx.get("access"), "GET", "/users/me")))
    step("auth_oauth_session")(lambda: body((200, sessions_of("bench0129"), None)))
    step("auth_oauth_refresh")(lambda: exchange({
        "grant_type": "refresh_token", "client_id": ctx.get("app", "x"), "client_secret": ctx.get("app_secret"),
        "refresh_token": ctx.get("refresh", "x"), "redirect_uri": CB}))
    step("auth_oauth_use_refreshed")(lambda: body(call(ctx.get("access"), "GET", "/users/me")))
    step("auth_oauth_deauthorize")(lambda: body(raw("POST", "/oauth/deauthorize", {
        "Authorization": "Bearer " + str(ctx.get("o1")), "Content-Type": "application/json"},
        json.dumps({"client_id": ctx.get("app", "x")}))))
    step("auth_oauth_after_deauth")(lambda: body(call(ctx.get("access"), "GET", "/users/me")))
    step("auth_oauth_delete_app")(lambda: body(call(admin, "DELETE", f"/oauth/apps/{ctx.get('app', 'x')}")))

    # ---- GitLab SSO up to the redirect ----
    def gitlab(path):
        st, v, h = raw("GET", path)
        loc = urllib.parse.urlparse(h.get("Location") or "")
        q = urllib.parse.parse_qs(loc.query)
        state = json.loads(base64.b64decode(q["state"][0] + "==").decode()) if "state" in q else None
        cookies = sorted(c.split("=")[0] for c in h.get_all("Set-Cookie") or [])
        toks = psql("SELECT Type, length(Token), Extra FROM Tokens WHERE Type NOT IN "
                    "('password_recovery', 'verify_email') ORDER BY CreateAt").strip().splitlines()
        return body((st, {"host": loc.netloc, "path": loc.path, "query": {k: v[0] for k, v in q.items() if k != "state"},
                          "state": sorted(state) if isinstance(state, dict) else state,
                          "state_action": state.get("action") if isinstance(state, dict) else None,
                          "cookies": cookies, "tokens": toks}, h))
    step("auth_gitlab_login")(lambda: gitlab("/oauth/gitlab/login?redirect_to=%2Fbench%2Fchannels%2Ftown-square"))
    step("auth_gitlab_mobile")(lambda: gitlab("/oauth/gitlab/mobile_login?redirect_to=mmauth%3A%2F%2Fcallback"))
    step("auth_gitlab_signup")(lambda: gitlab("/oauth/gitlab/signup"))
    step("auth_gitlab_complete_bad_state")(lambda: (lambda r: body((r[0], {"location": r[2].get("Location")}, r[2])))(
        raw("GET", "/signup/gitlab/complete?code=abc&state=bm9wZQ")))

    # ---- CSRF on cookie sessions ----
    def cookie_login(xrw):
        st, v, h = call_hdr("POST", "/users/login", {"login_id": "bench0130", "password": PW},
                            {"X-Requested-With": "XMLHttpRequest"} if xrw else {})
        jar = {}
        for c in h.get_all("Set-Cookie") or []:
            k, _, rest = c.partition("=")
            jar[k] = rest.split(";")[0]
        ctx["jar"] = jar
        return body((st, {"cookies": sorted(jar)}, h))
    step("auth_csrf_login_plain")(lambda: cookie_login(False))
    step("auth_csrf_login")(lambda: cookie_login(True))

    def call_hdr(method, path, data, extra):
        return raw(method, "/api/v4" + path, {"Content-Type": "application/json", **extra}, json.dumps(data))

    def cookie_call(method, path, extra=None, data=None):
        jar = ctx.get("jar", {})
        hd = {"Cookie": "; ".join(f"{k}={v}" for k, v in jar.items()), "Content-Type": "application/json", **(extra or {})}
        st, v, h = raw(method, "/api/v4" + path, hd, None if data is None else json.dumps(data))
        cleared = sorted(c.split("=")[0] for c in h.get_all("Set-Cookie") or [])
        return body((st, {"answer": v, "set_cookie": cleared}, h))
    me = {"nickname": "csrf"}
    step("auth_csrf_get")(lambda: cookie_call("GET", "/users/me"))
    step("auth_csrf_xrw")(lambda: cookie_call("PUT", "/users/me/patch", {"X-Requested-With": "XMLHttpRequest"}, me))
    step("auth_csrf_token")(lambda: cookie_call("PUT", "/users/me/patch", {"X-CSRF-Token": ctx.get("jar", {}).get("MMCSRF", "")}, me))
    step("auth_csrf_wrong_token")(lambda: cookie_call("PUT", "/users/me/patch", {"X-CSRF-Token": "wrong"}, me))
    step("auth_csrf_none")(lambda: cookie_call("PUT", "/users/me/patch", None, me))
    step("auth_csrf_after")(lambda: cookie_call("GET", "/users/me"))
    step("auth_csrf_logout")(lambda: cookie_call("POST", "/users/logout", {"X-Requested-With": "XMLHttpRequest"}))
