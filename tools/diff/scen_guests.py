"""Guest accounts: a user who is in several channels and two teams is made a guest, looks around, gets websocket
events about others, is added to and removed from a channel and a team, then promoted back.

Neither server demotes without a license (POST /users/{id}/demote: 501 api.team.demote_user_to_guest.license.error)
and a guest cannot log in with a password without one, so the demotion is done the way Go's store does it
(SqlUserStore.DemoteUserToGuest: Users.Roles, ChannelMembers and TeamMembers scheme flags; then
UpdateSessionsIsGuest: Sessions.Roles and Props.is_guest), straight in each server's database, followed by
POST /caches/invalidate so the servers forget what they cached. The guest's password session, made before, is then
a guest session, as it would be after a licensed demotion. Promotion (POST /users/{id}/promote) needs no license and
runs through the API. G is bench0300; before the demotion it is left in one channel of the bench team, bench-03,
which it shares with u1 (bench0101) but not with u2 (bench0102)."""

import sqldiff


def register(s):
    step, call, keep, ctx, body, psql = s.step, s.call, s.keep, s.ctx, s.body, s.psql
    u1, u1_id, u2, u2_id, u3_id = s.u1, s.u1_id, s.u2, s.u2_id, s.u3_id
    admin, team, public = s.admin, s.team, s.public

    def chan(name):
        return psql(f"SELECT Id FROM Channels WHERE Name = '{name}' AND TeamId = '{team}'").strip()

    bench03, bench04, bench11 = chan("bench-03"), chan("bench-04"), chan("bench-11")
    town = chan("town-square")
    g = {}

    def ok(v=None):
        return 200, v, None

    def gcall(method, path, data=None):
        return call(g["tok"], method, path, data)

    def r(name, method, path, data=None, unordered=(), who="g"):
        def run():
            tok = g["tok"] if who == "g" else {"u1": u1, "u2": u2, "admin": admin}[who]
            return body(call(tok, method, path, data), unordered)
        step(f"guests_{name}")(run)

    # ---- setup: guest accounts on, G logged in (a regular session) with a websocket, a second team ----
    def enable():
        st, v, h = call(admin, "PUT", "/config/patch", {"GuestAccountsSettings": {"Enable": True}})
        return body((st, (v or {}).get("GuestAccountsSettings", v), h))
    step("guests_config_enable")(enable)

    def login():
        g["tok"], g["id"] = s.login("bench0300")
        s.observers.append(sqldiff.Observer("guest", g["tok"], s.state))
        return ok()
    step("guests_login")(login)
    step("guests_team2_create")(lambda: keep("t2", call(admin, "POST", "/teams",
                                                        {"name": "guests-two", "display_name": "Guests two", "type": "O"})))
    step("guests_team2_add")(lambda: call(admin, "POST", f"/teams/{ctx['t2']}/members", {"team_id": ctx["t2"], "user_id": g["id"]}))

    # ---- demote: no license on either side ----
    step("guests_demote_api")(lambda: call(admin, "POST", f"/users/{g['id']}/demote"))
    step("guests_demote_api_user")(lambda: call(u1, "POST", f"/users/{g['id']}/demote"))
    step("guests_promote_not_guest")(lambda: call(admin, "POST", f"/users/{g['id']}/promote"))

    def demote_db():
        gid = g["id"]
        psql(f"DELETE FROM ChannelMembers WHERE UserId = '{gid}' AND ChannelId IN "
             f"(SELECT Id FROM Channels WHERE TeamId = '{team}' AND Id <> '{bench03}')")
        psql(f"UPDATE Users SET Roles = 'system_guest', UpdateAt = (extract(epoch from now()) * 1000)::bigint WHERE Id = '{gid}'")
        psql(f"UPDATE ChannelMembers SET SchemeUser = false, SchemeAdmin = false, SchemeGuest = true WHERE UserId = '{gid}'")
        psql(f"UPDATE TeamMembers SET SchemeUser = false, SchemeAdmin = false, SchemeGuest = true WHERE UserId = '{gid}'")
        psql(f"UPDATE Sessions SET Roles = 'system_guest' WHERE UserId = '{gid}'")
        psql(f"UPDATE Sessions SET Props = jsonb_set(Props, '{{is_guest}}', '\"true\"') WHERE UserId = '{gid}'")
        return ok()
    step("guests_demote_db")(demote_db)
    step("guests_caches_invalidate")(lambda: call(admin, "POST", "/caches/invalidate"))

    # ---- the guest, as it sees itself ----
    r("me", "GET", "/users/me")
    r("sessions", "GET", "/users/me/sessions", unordered=("",))
    r("my_teams", "GET", "/users/me/teams", unordered=("",))
    r("my_team_members", "GET", "/users/me/teams/members", unordered=("",))
    r("my_channels", "GET", f"/users/me/teams/{team}/channels", unordered=("",))
    r("my_channel_member", "GET", f"/channels/{bench03}/members/me")
    r("roles", "POST", "/roles/names", ["system_guest", "team_guest", "channel_guest"], unordered=("",))
    step("guests_password_login")(lambda: call(None, "POST", "/users/login",
                                                 {"login_id": "bench0300", "password": s.password}))

    # ---- the guest's view of other users ----
    r("users", "GET", "/users?page=0&per_page=200", unordered=("",))
    r("users_in_team", "GET", f"/users?in_team={team}&page=0&per_page=200", unordered=("",))
    r("users_in_channel", "GET", f"/users?in_channel={bench03}&page=0&per_page=200", unordered=("",))
    r("users_in_channel_not_member", "GET", f"/users?in_channel={public}&page=0&per_page=60")
    r("users_not_in_channel", "GET", f"/users?in_team={team}&not_in_channel={bench03}&page=0&per_page=60")
    r("users_search", "POST", "/users/search", {"term": "bench01", "team_id": team}, unordered=("",))
    r("users_search_all", "POST", "/users/search", {"term": "bench010"}, unordered=("",))
    r("autocomplete", "GET", f"/users/autocomplete?in_team={team}&name=bench010")
    r("autocomplete_channel", "GET", f"/users/autocomplete?in_team={team}&in_channel={bench03}&name=bench010")
    r("users_ids", "POST", "/users/ids", [u1_id, u2_id, u3_id], unordered=("",))
    r("users_usernames", "POST", "/users/usernames", ["bench0101", "bench0102"], unordered=("",))
    r("status_ids", "POST", "/users/status/ids", [u1_id, u2_id], unordered=("",))
    r("user_shared", "GET", f"/users/{u1_id}")
    r("user_unshared", "GET", f"/users/{u2_id}")
    r("user_unshared_by_name", "GET", "/users/username/bench0102")
    r("user_unshared_status", "GET", f"/users/{u2_id}/status")
    step("guests_user_unshared_image")(lambda: call(g["tok"], "GET", f"/users/{u2_id}/image"))
    r("users_stats", "GET", "/users/stats")
    r("known_users", "GET", "/users/known", unordered=("",))

    # ---- teams and channels ----
    r("teams", "GET", "/teams", unordered=("",))
    r("team", "GET", f"/teams/{team}")
    r("team_members", "GET", f"/teams/{team}/members?page=0&per_page=200", unordered=("",))
    r("team_member_unshared", "GET", f"/teams/{team}/members/{u2_id}")
    r("team_members_ids", "POST", f"/teams/{team}/members/ids", [u1_id, u2_id], unordered=("",))
    r("team_stats", "GET", f"/teams/{team}/stats")
    r("public_channels", "GET", f"/teams/{team}/channels?page=0&per_page=60")
    r("channels_search", "POST", f"/teams/{team}/channels/search", {"term": "bench-0"})
    r("channel_not_member", "GET", f"/channels/{bench04}")
    r("channel_by_name_not_member", "GET", f"/teams/{team}/channels/name/bench-04")
    r("channel_members", "GET", f"/channels/{bench03}/members?page=0&per_page=200", unordered=("",))
    r("channel_members_ids", "POST", f"/channels/{bench03}/members/ids", [u1_id, u2_id], unordered=("",))
    r("channel_members_not_member", "GET", f"/channels/{bench04}/members?page=0&per_page=60")
    r("channel_stats", "GET", f"/channels/{bench03}/stats")
    step("guests_join_open")(lambda: gcall("POST", f"/channels/{bench04}/members", {"user_id": g["id"]}))
    step("guests_join_town_square")(lambda: gcall("POST", f"/channels/{town}/members", {"user_id": g["id"]}))
    step("guests_create_channel")(lambda: gcall("POST", "/channels", {"team_id": team, "name": "guest-made",
                                                                       "display_name": "Guest made", "type": "O"}))
    step("guests_create_private")(lambda: gcall("POST", "/channels", {"team_id": team, "name": "guest-private",
                                                                       "display_name": "Guest private", "type": "P"}))
    step("guests_add_other")(lambda: gcall("POST", f"/channels/{bench03}/members", {"user_id": u2_id}))
    step("guests_dm_unshared")(lambda: gcall("POST", "/channels/direct", [g["id"], u2_id]))
    step("guests_dm_shared")(lambda: keep("gdm", gcall("POST", "/channels/direct", [g["id"], u1_id])))
    step("guests_gm_unshared")(lambda: gcall("POST", "/channels/group", [g["id"], u1_id, u2_id]))
    step("guests_post_dm")(lambda: gcall("POST", "/posts", {"channel_id": ctx.get("gdm", "x" * 26), "message": "hi from a guest"}))
    step("guests_post_channel")(lambda: gcall("POST", "/posts", {"channel_id": bench03, "message": "guest says @bench0102 @all"}))
    step("guests_invite_team")(lambda: gcall("POST", f"/teams/{team}/members", {"team_id": team, "user_id": u2_id}))
    step("guests_join_team2_open")(lambda: gcall("POST", f"/teams/{ctx['t2']}/members", {"team_id": ctx["t2"], "user_id": g["id"]}))

    # ---- websocket events the guest gets about others ----
    step("guests_ws_shared_updated")(lambda: call(u1, "PUT", f"/users/{u1_id}/patch", {"nickname": "shared"}))
    step("guests_ws_unshared_updated")(lambda: call(u2, "PUT", f"/users/{u2_id}/patch", {"nickname": "unshared"}))
    step("guests_ws_post_shared")(lambda: call(u1, "POST", "/posts", {"channel_id": bench03, "message": "to the guest's channel"}))
    step("guests_ws_post_other")(lambda: call(u2, "POST", "/posts", {"channel_id": s.bench00, "message": "not for the guest"}))
    step("guests_ws_status_unshared")(lambda: call(u2, "PUT", f"/users/{u2_id}/status", {"user_id": u2_id, "status": "away"}))
    step("guests_ws_status_shared")(lambda: call(u1, "PUT", f"/users/{u1_id}/status", {"user_id": u1_id, "status": "away"}))
    step("guests_ws_unshared_picture_reset")(lambda: call(admin, "DELETE", f"/users/{u2_id}/image"))

    # ---- what others see of the guest ----
    step("guests_u1_sees_guest")(lambda: body(call(u1, "GET", f"/users/{g['id']}")))
    step("guests_admin_guest_filter")(lambda: body(call(admin, "GET", f"/users?in_team={team}&role=system_guest&page=0&per_page=60")))
    step("guests_team_stats_admin")(lambda: body(call(admin, "GET", f"/teams/{team}/stats")))
    step("guests_channel_stats_admin")(lambda: body(call(admin, "GET", f"/channels/{bench03}/stats")))

    # ---- adding the guest to a channel and a team, and removing it ----
    step("guests_add_to_channel_by_u1")(lambda: call(u1, "POST", f"/channels/{bench11}/members", {"user_id": g["id"]}))
    step("guests_add_to_channel")(lambda: body(call(admin, "POST", f"/channels/{bench04}/members", {"user_id": g["id"]})))
    step("guests_channel_member_after_add")(lambda: body(gcall("GET", f"/channels/{bench04}/members/me")))
    step("guests_users_after_add")(lambda: body(gcall("POST", "/users/ids", [u1_id, u2_id, u3_id]), ("",)))
    step("guests_remove_from_channel")(lambda: call(admin, "DELETE", f"/channels/{bench04}/members/{g['id']}"))
    step("guests_leave_channel")(lambda: gcall("DELETE", f"/channels/{bench03}/members/{g['id']}"))
    step("guests_rejoin_channel")(lambda: gcall("POST", f"/channels/{bench03}/members", {"user_id": g["id"]}))
    step("guests_readd_channel")(lambda: call(admin, "POST", f"/channels/{bench03}/members", {"user_id": g["id"]}))
    step("guests_team2_remove")(lambda: call(admin, "DELETE", f"/teams/{ctx['t2']}/members/{g['id']}"))
    step("guests_team2_readd")(lambda: body(call(admin, "POST", f"/teams/{ctx['t2']}/members",
                                                 {"team_id": ctx["t2"], "user_id": g["id"]})))
    step("guests_team2_channels")(lambda: body(gcall("GET", f"/users/me/teams/{ctx['t2']}/channels"), ("",)))
    step("guests_team2_add_batch_u1")(lambda: call(u1, "POST", f"/teams/{team}/members/batch",
                                                   [{"team_id": team, "user_id": g["id"]}]))
    step("guests_team_member_roles")(lambda: call(admin, "PUT", f"/teams/{team}/members/{g['id']}/schemeRoles",
                                                  {"scheme_user": True, "scheme_admin": False, "scheme_guest": False}))
    step("guests_channel_member_roles")(lambda: call(admin, "PUT", f"/channels/{bench03}/members/{g['id']}/schemeRoles",
                                                     {"scheme_user": True, "scheme_admin": False, "scheme_guest": False}))

    # ---- guest restricted domains ----
    step("guests_domains_set")(lambda: call(admin, "PUT", "/config/patch",
                                            {"GuestAccountsSettings": {"RestrictCreationToDomains": "guests.example.org"}}))
    step("guests_email_outside")(lambda: gcall("PUT", f"/users/{g['id']}/patch",
                                               {"email": "bench0300@elsewhere.org", "password": s.password}))
    step("guests_email_inside")(lambda: body(gcall("PUT", f"/users/{g['id']}/patch",
                                                   {"email": "bench0300@guests.example.org", "password": s.password})))
    step("guests_admin_email_outside")(lambda: call(admin, "PUT", f"/users/{g['id']}/patch", {"email": "bench0300@other.example.com"}))
    step("guests_team_domains_set")(lambda: call(admin, "PUT", f"/teams/{ctx['t2']}/patch", {"allowed_domains": "example.com"}))
    step("guests_team2_remove_again")(lambda: call(admin, "DELETE", f"/teams/{ctx['t2']}/members/{g['id']}"))
    step("guests_team2_add_domain")(lambda: call(admin, "POST", f"/teams/{ctx['t2']}/members",
                                                 {"team_id": ctx["t2"], "user_id": g["id"]}))
    step("guests_domains_clear")(lambda: call(admin, "PUT", "/config/patch",
                                              {"GuestAccountsSettings": {"RestrictCreationToDomains": ""}}))

    # ---- promote back ----
    step("guests_promote_by_user")(lambda: call(u1, "POST", f"/users/{g['id']}/promote"))
    step("guests_promote")(lambda: call(admin, "POST", f"/users/{g['id']}/promote"))
    step("guests_promoted_mentions_db")(lambda: body(ok(psql(
        "SELECT COALESCE(t.Name, '-') || '/' || c.Name || ':' || cm.MentionCount || ':' || cm.MsgCount || ':' "
        "|| cm.SchemeGuest || ':' || cm.SchemeUser FROM ChannelMembers cm "
        f"JOIN Channels c ON c.Id = cm.ChannelId LEFT JOIN Teams t ON t.Id = c.TeamId WHERE cm.UserId = '{g['id']}' "
        "ORDER BY 1").split())))
    r("promoted_me", "GET", "/users/me")
    r("promoted_sessions", "GET", "/users/me/sessions", unordered=("",))
    r("promoted_team_members", "GET", "/users/me/teams/members", unordered=("",))
    r("promoted_channel_member", "GET", f"/channels/{bench03}/members/me")
    r("promoted_channels", "GET", f"/users/me/teams/{team}/channels", unordered=("",))
    r("promoted_users_ids", "POST", "/users/ids", [u1_id, u2_id, u3_id], unordered=("",))
    step("guests_promoted_join_open")(lambda: gcall("POST", f"/channels/{bench04}/members", {"user_id": g["id"]}))
    step("guests_promoted_ws_unshared_updated")(lambda: call(u2, "PUT", f"/users/{u2_id}/patch", {"nickname": "seen"}))
    step("guests_promoted_password_login")(lambda: call(None, "POST", "/users/login",
                                                          {"login_id": "bench0300@guests.example.org", "password": s.password}))
    step("guests_promote_again")(lambda: call(admin, "POST", f"/users/{g['id']}/promote"))
