"""Teams, users and admin for sqldiff.py: teams (open and invite-only, patch, privacy, invite id, members and their
roles, leave, unreads, search, archive and restore), user edits, search and autocomplete, stats, passwords,
sessions, personal access tokens, bots, roles, preferences in batches and the admin user lists.

Personal access tokens and bots are switched on through the API at the start (each server has its own copy of the
config and of the database)."""

PW2 = "Bench-pass-456"


def register(s):
    step, call, ctx = s.step, s.call, s.ctx
    u1, u1_id, u2, u2_id, u3_id = s.u1, s.u1_id, s.u2, s.u2_id, s.u3_id
    admin, admin_id, spare_id = s.admin, s.admin_id, s.spare_id
    team, public, bench00 = s.team, s.public, s.bench00
    tok = {"u3": s.u3}
    X = "x" * 26

    def keep(k, r, field="id"):
        st, v, _ = r
        if isinstance(v, dict) and field in v and st < 300:
            ctx[k] = v[field]
        return r

    def digest(r):
        """A list's content as the answer's keys (sqldiff compares keys only): how many, and the first names."""
        st, v, h = r
        if isinstance(v, dict) and "users" in v:
            v = v["users"] + [{"username": "+" + u["username"]} for u in v.get("out_of_channel") or []]
        if isinstance(v, list):
            names = [str(u.get("username") or u.get("name") or u.get("description") or "?") if isinstance(u, dict) else str(u) for u in v]
            return st, {f"n={len(v)}": 0, "first=" + ",".join(names[:5]): 0}, h
        if isinstance(v, dict) and "total_users_count" in v:
            return st, {f"total={v['total_users_count']}": 0}, h
        return r

    def D(*a, **k):
        return digest(call(*a, **k))

    def c(k):
        return ctx.get(k, X)

    # ---- config: personal access tokens and bots ----
    step("adm_config_patch")(lambda: call(admin, "PUT", "/config/patch", {"ServiceSettings": {
        "EnableUserAccessTokens": True, "EnableBotAccountCreation": True}}))

    # ---- teams ----
    step("adm_team_create_open")(lambda: keep("t1", call(u1, "POST", "/teams",
                                                         {"name": "sqldiff-open", "display_name": "SQL diff open", "type": "O"})))
    step("adm_team_create_invite")(lambda: keep("t2", call(u1, "POST", "/teams",
                                                           {"name": "sqldiff-inv", "display_name": "SQL diff invite", "type": "I"})))
    step("adm_team_get")(lambda: call(u1, "GET", f"/teams/{c('t1')}"))
    step("adm_team_exists")(lambda: call(u1, "GET", "/teams/name/sqldiff-open/exists"))
    step("adm_team_patch")(lambda: call(u1, "PUT", f"/teams/{c('t1')}/patch",
                                        {"display_name": "SQL diff open 2", "description": "a team", "allow_open_invite": True}))
    step("adm_team_update")(lambda: team_update())

    def team_update():
        st, t, _ = call(u1, "GET", f"/teams/{c('t1')}")
        t["company_name"] = "ACME"
        t["description"] = "updated"
        return call(u1, "PUT", f"/teams/{c('t1')}", t)

    step("adm_team_privacy_inv")(lambda: call(u1, "PUT", f"/teams/{c('t1')}/privacy", {"privacy": "I"}))
    step("adm_team_privacy_open")(lambda: call(u1, "PUT", f"/teams/{c('t1')}/privacy", {"privacy": "O"}))
    step("adm_team_regen_invite")(lambda: keep("invite", call(u1, "POST", f"/teams/{c('t2')}/regenerate_invite_id"),
                                               "invite_id"))
    step("adm_team_by_invite")(lambda: call(u2, "GET", f"/teams/invite/{c('invite')}"))
    step("adm_team_join_invite")(lambda: call(u2, "POST", f"/teams/members/invite?invite_id={c('invite')}"))
    step("adm_team_self_join_inv")(lambda: call(s.u3, "POST", f"/teams/{c('t2')}/members", {"team_id": c("t2"), "user_id": u3_id}))
    step("adm_team_self_join_open")(lambda: call(tok["u3"], "POST", f"/teams/{c('t1')}/members", {"team_id": c("t1"), "user_id": u3_id}))
    step("adm_team_add_member")(lambda: call(u1, "POST", f"/teams/{c('t2')}/members", {"team_id": c("t2"), "user_id": u3_id}))
    step("adm_team_add_batch")(lambda: call(u1, "POST", f"/teams/{c('t2')}/members/batch",
                                            [{"team_id": c("t2"), "user_id": spare_id}, {"team_id": c("t2"), "user_id": admin_id}]))
    step("adm_team_members")(lambda: D(u1, "GET", f"/teams/{c('t2')}/members?page=0&per_page=60"))
    step("adm_team_member")(lambda: call(u1, "GET", f"/teams/{c('t2')}/members/{u2_id}"))
    step("adm_team_members_ids")(lambda: call(u1, "POST", f"/teams/{c('t2')}/members/ids", [u2_id, u3_id]))
    step("adm_team_member_admin")(lambda: call(u1, "PUT", f"/teams/{c('t2')}/members/{u2_id}/schemeRoles",
                                               {"scheme_user": True, "scheme_admin": True}))
    step("adm_team_member_roles")(lambda: call(u1, "PUT", f"/teams/{c('t2')}/members/{u3_id}/roles",
                                               {"roles": "team_user team_admin"}))
    step("adm_team_member_user")(lambda: call(u1, "PUT", f"/teams/{c('t2')}/members/{u2_id}/schemeRoles",
                                              {"scheme_user": True, "scheme_admin": False}))
    step("adm_team_stats")(lambda: call(u1, "GET", f"/teams/{c('t2')}/stats"))
    step("adm_team_post")(lambda: team_post())

    def team_post():
        st, chs, _ = call(u1, "GET", f"/users/me/teams/{c('t2')}/channels")
        town = next((ch["id"] for ch in chs or [] if ch.get("name") == "town-square"), X)
        ctx["t2town"] = town
        return call(u1, "POST", "/posts", {"channel_id": town, "message": "team hello"})

    step("adm_team_unread_all")(lambda: call(u2, "GET", "/users/me/teams/unread"))
    step("adm_team_unread_one")(lambda: call(u2, "GET", f"/users/{u2_id}/teams/{c('t2')}/unread"))
    step("adm_team_leave")(lambda: call(tok["u3"], "DELETE", f"/teams/{c('t2')}/members/{u3_id}"))
    step("adm_team_remove")(lambda: call(u1, "DELETE", f"/teams/{c('t2')}/members/{spare_id}"))
    step("adm_team_search_user")(lambda: D(u1, "POST", "/teams/search", {"term": "sqldiff"}))
    step("adm_team_search_admin")(lambda: D(admin, "POST", "/teams/search",
                                               {"term": "sqldiff", "page": 0, "per_page": 10, "allow_open_invite": True}))
    step("adm_teams_list")(lambda: D(admin, "GET", "/teams?page=0&per_page=50&include_total_count=true"))
    step("adm_team_archive")(lambda: call(u1, "DELETE", f"/teams/{c('t1')}"))
    step("adm_teams_deleted")(lambda: D(admin, "GET", "/teams?page=0&per_page=50&include_deleted=true"))
    step("adm_team_search_deleted")(lambda: D(admin, "POST", "/teams/search", {"term": "sqldiff", "include_deleted": True}))
    step("adm_team_restore")(lambda: call(admin, "POST", f"/teams/{c('t1')}/restore"))
    step("adm_my_teams_after")(lambda: call(u1, "GET", "/users/me/teams"))

    # ---- users ----
    step("adm_user_patch_names")(lambda: call(u1, "PUT", f"/users/{u1_id}/patch",
                                              {"first_name": "Sql", "last_name": "Diff", "nickname": "sd", "position": "qa", "locale": "es"}))
    step("adm_user_patch_tz")(lambda: call(u1, "PUT", f"/users/{u1_id}/patch", {"timezone": {
        "useAutomaticTimezone": "false", "manualTimezone": "Europe/Madrid", "automaticTimezone": ""}}))
    step("adm_user_patch_notify")(lambda: call(u1, "PUT", f"/users/{u1_id}/patch", {"notify_props": {
        "email": "true", "push": "all", "desktop": "mention", "mention_keys": "sqldiff", "channel": "true",
        "first_name": "true", "comments": "any", "push_status": "away", "desktop_sound": "true"}}))
    step("adm_user_patch_by_admin")(lambda: call(admin, "PUT", f"/users/{spare_id}/patch", {"nickname": "spare", "locale": "en"}))
    step("adm_user_update")(lambda: user_update())

    def user_update():
        st, u, _ = call(u1, "GET", "/users/me")
        u["position"] = "lead"
        u["locale"] = "en"
        return call(u1, "PUT", f"/users/{u1_id}", u)

    step("adm_user_search_team")(lambda: D(u1, "POST", "/users/search", {"term": "bench010", "team_id": team}))
    step("adm_user_search_chan")(lambda: D(u1, "POST", "/users/search", {"term": "bench", "in_channel_id": bench00, "limit": 20}))
    step("adm_user_search_notch")(lambda: D(u1, "POST", "/users/search",
                                               {"term": "bench01", "team_id": team, "not_in_channel_id": bench00}))
    step("adm_user_search_notteam")(lambda: D(admin, "POST", "/users/search", {"term": "bench01", "not_in_team_id": c("t2")}))
    step("adm_user_search_inactive")(lambda: D(admin, "POST", "/users/search", {"term": "bench", "allow_inactive": True, "limit": 5}))
    step("adm_user_search_role")(lambda: D(admin, "POST", "/users/search", {"term": "bench", "role": "system_admin"}))
    step("adm_user_search_name")(lambda: D(u1, "POST", "/users/search", {"term": "Sql Diff"}))
    step("adm_autocomplete_chan")(lambda: D(u1, "GET", f"/users/autocomplete?in_team={team}&in_channel={bench00}&name=bench01"))
    step("adm_autocomplete_team")(lambda: D(u1, "GET", f"/users/autocomplete?in_team={team}&name=sql&limit=10"))
    step("adm_autocomplete_all")(lambda: D(u1, "GET", "/users/autocomplete?name=bench049"))
    step("adm_users_stats")(lambda: D(admin, "GET", "/users/stats"))
    step("adm_users_stats_filtered")(lambda: D(admin, "GET", f"/users/stats/filtered?in_team={team}&include_deleted=true&roles=system_user"))
    step("adm_user_search_roles")(lambda: D(admin, "POST", "/users/search",
                                            {"term": "bench", "team_id": c("t2"), "roles": ["system_user"], "team_roles": ["team_admin"]}))
    step("adm_user_search_chroles")(lambda: D(admin, "POST", "/users/search",
                                              {"term": "bench", "in_channel_id": bench00, "channel_roles": ["channel_user"], "limit": 5}))
    step("adm_users_stats_troles")(lambda: D(admin, "GET", f"/users/stats/filtered?in_team={c('t2')}&team_roles=team_admin,team_user"))
    step("adm_users_stats_croles")(lambda: D(admin, "GET", f"/users/stats/filtered?in_channel={bench00}&channel_roles=channel_admin"))
    step("adm_users_stats_sysroles")(lambda: D(admin, "GET", "/users/stats/filtered?roles=system_admin"))
    step("adm_users_stats_badrole")(lambda: D(admin, "GET", "/users/stats/filtered?roles=Bad-Role"))
    step("adm_users_stats_user")(lambda: D(u1, "GET", "/users/stats"))

    # ---- passwords and sessions ----
    step("adm_password_own")(lambda: call(tok["u3"], "PUT", f"/users/{u3_id}/password",
                                          {"current_password": s.password, "new_password": PW2}))
    step("adm_password_wrong")(lambda: call(tok["u3"], "PUT", f"/users/{u3_id}/password",
                                            {"current_password": "nope-nope-1", "new_password": PW2}))
    step("adm_password_admin")(lambda: call(admin, "PUT", f"/users/{u3_id}/password", {"new_password": s.password}))
    step("adm_relogin")(lambda: relogin())

    def relogin():
        st, v, h = call(None, "POST", "/users/login", {"login_id": "bench0103", "password": s.password})
        if st == 200:
            tok["u3"] = h["Token"]
        return st, v, h

    step("adm_second_login")(lambda: second_login())

    def second_login():
        st, v, h = call(None, "POST", "/users/login", {"login_id": "bench0103", "password": s.password})
        if st == 200:
            tok["u3b"] = h["Token"]
        return st, v, h

    step("adm_sessions")(lambda: sessions())

    def sessions():
        r = call(tok["u3"], "GET", f"/users/{u3_id}/sessions")
        # the newest session is the second login
        lst = sorted(r[1] or [], key=lambda x: x.get("create_at", 0))
        if lst:
            ctx["sess"] = lst[-1]["id"]
        return r

    step("adm_session_revoke")(lambda: call(tok["u3"], "POST", f"/users/{u3_id}/sessions/revoke", {"session_id": c("sess")}))
    step("adm_session_revoked_use")(lambda: call(tok.get("u3b"), "GET", "/users/me"))
    step("adm_sessions_admin")(lambda: call(admin, "GET", f"/users/{u3_id}/sessions"))

    # ---- personal access tokens ----
    step("adm_pat_role")(lambda: call(admin, "PUT", f"/users/{u3_id}/roles", {"roles": "system_user system_user_access_token"}))
    step("adm_pat_create")(lambda: pat_create())

    def pat_create():
        r = call(tok["u3"], "POST", f"/users/{u3_id}/tokens", {"description": "sqldiff"})
        st, v, _ = r
        if st < 300 and v:
            ctx["pat"], ctx["pat_token"] = v["id"], v["token"]
        return r

    step("adm_pat_admin_create")(lambda: call(admin, "POST", f"/users/{u2_id}/tokens", {"description": "by admin"}))
    step("adm_pat_use")(lambda: call(ctx.get("pat_token"), "GET", "/users/me"))
    step("adm_pat_list_own")(lambda: D(tok["u3"], "GET", f"/users/{u3_id}/tokens"))
    step("adm_pat_list_all")(lambda: D(admin, "GET", "/users/tokens?page=0&per_page=50"))
    step("adm_pat_get")(lambda: call(tok["u3"], "GET", f"/users/tokens/{c('pat')}"))
    step("adm_pat_search")(lambda: D(admin, "POST", "/users/tokens/search", {"term": "bench0103"}))
    step("adm_pat_disable")(lambda: call(tok["u3"], "POST", "/users/tokens/disable", {"token_id": c("pat")}))
    step("adm_pat_use_disabled")(lambda: call(ctx.get("pat_token"), "GET", "/users/me"))
    step("adm_pat_enable")(lambda: call(tok["u3"], "POST", "/users/tokens/enable", {"token_id": c("pat")}))
    step("adm_pat_use_enabled")(lambda: call(ctx.get("pat_token"), "GET", "/users/me"))
    step("adm_pat_revoke")(lambda: call(tok["u3"], "POST", "/users/tokens/revoke", {"token_id": c("pat")}))
    step("adm_pat_use_revoked")(lambda: call(ctx.get("pat_token"), "GET", "/users/me"))
    step("adm_pat_role_back")(lambda: call(admin, "PUT", f"/users/{u3_id}/roles", {"roles": "system_user"}))

    # ---- bots ----
    step("adm_bot_create")(lambda: keep("bot", call(admin, "POST", "/bots",
                                                    {"username": "sqldiffbot", "display_name": "SQL diff bot", "description": "d"}),
                                        "user_id"))
    step("adm_bot_get")(lambda: call(admin, "GET", f"/bots/{c('bot')}"))
    step("adm_bots")(lambda: D(admin, "GET", "/bots?page=0&per_page=50"))
    step("adm_bot_patch")(lambda: call(admin, "PUT", f"/bots/{c('bot')}", {"display_name": "SQL diff bot 2", "description": "e"}))
    step("adm_bot_disable")(lambda: call(admin, "POST", f"/bots/{c('bot')}/disable"))
    step("adm_bots_disabled")(lambda: D(admin, "GET", "/bots?only_orphaned=false&include_deleted=true"))
    # Go refuses to enable it (its 250-user limit, known); Rust enables it, so disable it again on both.
    step("adm_bot_enable")(lambda: call(admin, "POST", f"/bots/{c('bot')}/enable"))
    step("adm_bot_disable_again")(lambda: call(admin, "POST", f"/bots/{c('bot')}/disable"))
    step("adm_bot_assign")(lambda: call(admin, "POST", f"/bots/{c('bot')}/assign/{u1_id}"))
    step("adm_bot_team_add")(lambda: call(admin, "POST", f"/teams/{team}/members", {"team_id": team, "user_id": c("bot")}))
    step("adm_user_to_bot")(lambda: keep("conv", call(admin, "POST", f"/users/{ctx.get('u110', X)}/convert_to_bot"), "user_id"))
    step("adm_bot_to_user")(lambda: call(admin, "POST", f"/bots/{ctx.get('u110', X)}/convert_to_user?set_system_admin=false",
                                         {"password": s.password, "username": "bench0110"}))
    step("adm_bot_to_user_login")(lambda: call(None, "POST", "/users/login", {"login_id": "bench0110", "password": s.password}))

    # ---- roles ----
    step("adm_role_by_name")(lambda: keep("role", call(u1, "GET", "/roles/name/team_user")))
    step("adm_roles_by_names")(lambda: call(u1, "POST", "/roles/names", ["system_user", "team_user", "channel_user"]))
    step("adm_role_get")(lambda: call(u1, "GET", f"/roles/{c('role')}"))
    step("adm_role_patch_user")(lambda: call(u1, "PUT", f"/roles/{c('role')}/patch", {"permissions": ["view_team"]}))
    step("adm_role_patch")(lambda: role_patch(False))
    step("adm_role_restore")(lambda: role_patch(True))

    def role_patch(restore):
        st, r, _ = call(admin, "GET", f"/roles/{c('role')}")
        perms = list((r or {}).get("permissions") or [])
        if not restore:
            ctx["role_perms"] = perms
            perms = [p for p in perms if p != "create_private_channel"]
        else:
            perms = ctx.get("role_perms", perms)
        return call(admin, "PUT", f"/roles/{c('role')}/patch", {"permissions": perms})

    # ---- preferences in batches ----
    step("adm_pref_save_batch")(lambda: call(u1, "PUT", f"/users/{u1_id}/preferences", [
        {"user_id": u1_id, "category": "display_settings", "name": "use_military_time", "value": "true"},
        {"user_id": u1_id, "category": "display_settings", "name": "channel_display_mode", "value": "centered"},
        {"user_id": u1_id, "category": "favorite_channel", "name": bench00, "value": "true"},
        {"user_id": u1_id, "category": "direct_channel_show", "name": u2_id, "value": "true"}]))
    step("adm_pref_category")(lambda: call(u1, "GET", f"/users/{u1_id}/preferences/display_settings"))
    step("adm_pref_one")(lambda: call(u1, "GET", f"/users/{u1_id}/preferences/display_settings/name/channel_display_mode"))
    step("adm_pref_delete_batch")(lambda: call(u1, "POST", f"/users/{u1_id}/preferences/delete", [
        {"user_id": u1_id, "category": "display_settings", "name": "use_military_time", "value": "true"},
        {"user_id": u1_id, "category": "favorite_channel", "name": bench00, "value": "true"},
        {"user_id": u1_id, "category": "direct_channel_show", "name": u2_id, "value": "true"}]))
    step("adm_pref_other_user")(lambda: call(u1, "PUT", f"/users/{u2_id}/preferences",
                                             [{"user_id": u2_id, "category": "display_settings", "name": "x", "value": "y"}]))

    # ---- admin user lists ----
    for name, q in [("inactive", "inactive=true"), ("active_false", "active=false"), ("active_true", "active=true&per_page=5"),
                    ("role_admin", "role=system_admin"), ("roles", "roles=system_admin,system_user_manager"),
                    ("team_sort_last", f"in_team={team}&sort=last_activity_at&per_page=5"),
                    ("team_sort_create", f"in_team={team}&sort=create_at&per_page=5"),
                    ("team_roles", f"in_team={team}&team_roles=team_admin"),
                    ("chan_sort_status", f"in_channel={bench00}&sort=status&per_page=5"),
                    ("chan_roles", f"in_channel={bench00}&channel_roles=channel_admin"),
                    ("not_in_team", "not_in_team={t2}&per_page=5"),
                    ("without_team", "without_team=true"),
                    ("not_in_chan", f"in_team={team}&not_in_channel={bench00}&per_page=5"),
                    ("group_constrained", f"in_team={team}&group_constrained=true"),
                    ("t2_team_roles", "in_team={t2}&team_roles=team_admin"),
                    ("t2_team_user", "in_team={t2}&team_roles=team_user&roles=system_user"),
                    ("chan_user", f"in_channel={bench00}&channel_roles=channel_user&per_page=5"),
                    ("unknown_role", "role=no_such_role"),
                    ("bad_roles", "roles=Bad-Role"),
                    ("unknown_team_role", "in_team={t2}&team_roles=no_such_role"),
                    ("team_roles_no_team", "team_roles=team_admin&per_page=5"),
                    ("bad_sort", "sort=bogus")]:
        step(f"adm_users_{name}")(lambda q=q: D(admin, "GET", "/users?" + q.replace("{t2}", c("t2"))))
    step("adm_users_user_inactive")(lambda: D(u1, "GET", "/users?inactive=true"))
    step("adm_user_teams")(lambda: call(admin, "GET", f"/users/{u2_id}/teams"))
    step("adm_user_team_members")(lambda: call(admin, "GET", f"/users/{u2_id}/teams/members"))

    # bench0110 for the convert steps (no one else uses it)
    ctx["u110"] = s.psql("SELECT Id FROM Users WHERE Username = 'bench0110'").strip()
