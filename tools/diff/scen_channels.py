"""Channels and the sidebar in depth: create, patch and update, members (paging, ids, stats, many at once, remove,
leave), member notify props and scheme roles, privacy, move between teams, archive (with and without webhooks),
restore, the archived and private lists, search and autocomplete, and the sidebar categories (create, rename,
order, moving channels, sorting/muted/collapsed, favorites, delete, where new channels and DMs land).

Steps whose values matter (counts, names, flags) return them as the answer's only key (see val), so the comparison
sees them; ids made during the run differ between the two servers and are left out."""

import json


def register(s):
    step, call, keep, ctx = s.step, s.call, s.keep, s.ctx
    u1, u1_id, u2, u2_id, u3, u3_id = s.u1, s.u1_id, s.u2, s.u2_id, s.u3, s.u3_id
    admin, team = s.admin, s.team

    many = [x for x in s.psql("SELECT Id FROM Users WHERE Username BETWEEN 'bench0110' AND 'bench0129' "
                              "ORDER BY Username").split()]
    town = s.psql(f"SELECT Id FROM Channels WHERE Name = 'town-square' AND TeamId = '{team}'").strip()
    gone = s.psql("SELECT Id FROM Users WHERE Username = 'bench0128'").strip()

    def val(r, f):
        """The answer reduced to what should match: f(answer) as the only key."""
        st, v, h = r
        if isinstance(st, int) and st < 300 and v is not None:
            try:
                return st, {json.dumps(f(v), sort_keys=True): 0}, h
            except Exception as e:  # noqa: BLE001
                return st, {"val failed " + repr(e)[:80]: 0}, h
        return r

    def chan(v):
        return {k: v.get(k) for k in ("name", "display_name", "type", "header", "purpose", "delete_at")
                if k != "delete_at"} | {"archived": bool(v.get("delete_at"))}

    def names(v):
        return sorted(c["name"] for c in v if c["name"].startswith("chan-") or c["name"].startswith("bench-0"))

    def chans(v):
        return sorted(c["name"] for c in v if c["name"].startswith("chan-"))

    def np(v):
        return v["notify_props"]

    def roles(v):
        return {k: v.get(k) for k in ("roles", "scheme_user", "scheme_admin", "scheme_guest")}

    def cats(v):
        out = []
        for c in v["categories"]:
            out.append({"type": c["type"], "display_name": c["display_name"], "sorting": c["sorting"],
                        "muted": c["muted"], "collapsed": c["collapsed"],
                        "mine": sorted(n for n in (cname.get(i) for i in c["channel_ids"]) if n),
                        "n": len(c["channel_ids"])})
        return {"order_len": len(v["order"]), "categories": out}

    def cat(v):
        return {"type": v["type"], "display_name": v["display_name"], "sorting": v["sorting"], "muted": v["muted"],
                "collapsed": v["collapsed"], "mine": sorted(n for n in (cname.get(i) for i in v["channel_ids"]) if n),
                "n": len(v["channel_ids"])}

    # channel id -> stable name, for the categories' channel lists
    cname = {}

    def keepc(k, r):
        keep(k, r)
        if k in ctx:
            cname[ctx[k]] = k
        return r

    cats_url = f"/users/{u1_id}/teams/{team}/channels/categories"

    def get_cats():
        return call(u1, "GET", cats_url)[1]

    def cat_of(kind):
        return next(c for c in get_cats()["categories"] if c["type"] == kind)

    # ---- create ----
    step("chan_create_open")(lambda: val(keepc("co", call(u1, "POST", "/channels", {
        "team_id": team, "name": "chan-open", "display_name": "Chan Open", "type": "O", "purpose": "p", "header": "h"})), chan))
    step("chan_create_private")(lambda: val(keepc("cp", call(u1, "POST", "/channels", {
        "team_id": team, "name": "chan-priv", "display_name": "Chan Priv", "type": "P"})), chan))
    step("chan_create_dup")(lambda: call(u1, "POST", "/channels", {
        "team_id": team, "name": "chan-open", "display_name": "Again", "type": "O"}))
    step("chan_create_badname")(lambda: call(u1, "POST", "/channels", {
        "team_id": team, "name": "Bad Name", "display_name": "Bad", "type": "O"}))
    step("chan_create_notype")(lambda: call(u1, "POST", "/channels", {
        "team_id": team, "name": "chan-notype", "display_name": "No type"}))
    step("chan_create_noteam")(lambda: call(u1, "POST", "/channels", {
        "team_id": "x" * 26, "name": "chan-noteam", "display_name": "No team", "type": "O"}))
    step("chan_get_by_name")(lambda: val(call(u2, "GET", f"/teams/{team}/channels/name/chan-open"), chan))
    step("chan_get_private_nonmember")(lambda: call(u2, "GET", f"/channels/{ctx['cp']}"))
    step("chan_new_in_category")(lambda: val(call(u1, "GET", cats_url), cats))

    # ---- patch, update ----
    step("chan_patch_header")(lambda: val(call(u1, "PUT", f"/channels/{ctx['co']}/patch", {"header": "new header"}), chan))
    step("chan_patch_purpose")(lambda: val(call(u1, "PUT", f"/channels/{ctx['co']}/patch", {"purpose": "new purpose"}), chan))
    step("chan_patch_display")(lambda: val(call(u1, "PUT", f"/channels/{ctx['co']}/patch", {"display_name": "Chan Open 2"}), chan))
    step("chan_patch_name")(lambda: val(call(u1, "PUT", f"/channels/{ctx['co']}/patch", {"name": "chan-open-2"}), chan))
    step("chan_patch_name_taken")(lambda: call(u1, "PUT", f"/channels/{ctx['co']}/patch", {"name": "bench-00"}))
    step("chan_patch_nonmember")(lambda: call(u2, "PUT", f"/channels/{ctx['co']}/patch", {"header": "intruder"}))
    step("chan_patch_private")(lambda: val(call(u1, "PUT", f"/channels/{ctx['cp']}/patch", {"header": "secret"}), chan))

    def update(token, k, **change):
        c = call(token, "GET", f"/channels/{ctx[k]}")[1]
        c.update(change)
        return call(token, "PUT", f"/channels/{ctx[k]}", c)

    step("chan_update")(lambda: val(update(u1, "co", purpose="updated purpose", header="updated header"), chan))
    step("chan_update_type")(lambda: update(u1, "co", type="P"))
    step("chan_update_empty_display")(lambda: update(u1, "co", display_name=""))

    # ---- members ----
    step("chan_add_many")(lambda: val(call(u1, "POST", f"/channels/{ctx['co']}/members", {"user_ids": [u2_id, u3_id] + many}),
                                      lambda v: len(v)))
    step("chan_add_again")(lambda: val(call(u1, "POST", f"/channels/{ctx['co']}/members", {"user_id": u2_id}), roles))
    step("chan_add_bad")(lambda: call(u1, "POST", f"/channels/{ctx['co']}/members", {"user_id": "x" * 26}))
    step("chan_add_neither")(lambda: call(u1, "POST", f"/channels/{ctx['co']}/members", {}))
    step("chan_members_p0")(lambda: val(call(u1, "GET", f"/channels/{ctx['co']}/members?page=0&per_page=10"), len))
    step("chan_members_p2")(lambda: val(call(u1, "GET", f"/channels/{ctx['co']}/members?page=2&per_page=10"), len))
    step("chan_members_p9")(lambda: val(call(u1, "GET", f"/channels/{ctx['co']}/members?page=9&per_page=10"), len))
    step("chan_members_ids")(lambda: val(call(u1, "POST", f"/channels/{ctx['co']}/members/ids", [u2_id, u3_id, s.spare_id]), len))
    step("chan_members_ids_empty")(lambda: call(u1, "POST", f"/channels/{ctx['co']}/members/ids", []))
    step("chan_member_get")(lambda: val(call(u1, "GET", f"/channels/{ctx['co']}/members/{u2_id}"), roles))
    step("chan_member_get_none")(lambda: call(u1, "GET", f"/channels/{ctx['co']}/members/{s.spare_id}"))
    stats = lambda v: {k: v.get(k) for k in ("member_count", "guest_count", "pinnedpost_count", "files_count")}  # noqa: E731
    step("chan_stats")(lambda: val(call(u1, "GET", f"/channels/{ctx['co']}/stats"), stats))
    step("chan_deactivate_member")(lambda: call(admin, "PUT", f"/users/{gone}/active", {"active": False}))
    step("chan_stats_deleted")(lambda: val(call(u1, "GET", f"/channels/{ctx['co']}/stats"), stats))
    step("chan_stats_excl_deleted")(lambda: val(call(u1, "GET", f"/channels/{ctx['co']}/stats?exclude_files_count=true"), stats))
    step("chan_members_users")(lambda: val(call(u1, "GET", f"/users?in_channel={ctx['co']}&page=0&per_page=100"), len))
    step("chan_members_users_active")(lambda: val(call(u1, "GET", f"/users?in_channel={ctx['co']}&active=true&page=0&per_page=100"), len))
    step("chan_member_counts")(lambda: val(call(u1, "POST", "/channels/stats/member_count", [ctx["co"], ctx["cp"]]),
                                           lambda v: sorted(v.values())))
    step("chan_remove_member")(lambda: call(u1, "DELETE", f"/channels/{ctx['co']}/members/{many[0]}"))
    step("chan_remove_nonmember")(lambda: call(u1, "DELETE", f"/channels/{ctx['co']}/members/{s.spare_id}"))
    step("chan_remove_by_member")(lambda: call(u3, "DELETE", f"/channels/{ctx['co']}/members/{many[1]}"))
    step("chan_stats_after")(lambda: val(call(u1, "GET", f"/channels/{ctx['co']}/stats"), stats))
    step("chan_timezones")(lambda: val(call(u1, "GET", f"/channels/{ctx['co']}/timezones"), len))

    # ---- notify props ----
    def props(token, uid, k, **p):
        return call(token, "PUT", f"/channels/{ctx[k]}/members/{uid}/notify_props", {"channel_id": ctx[k], "user_id": uid, **p})

    step("chan_mute")(lambda: props(u2, u2_id, "co", mark_unread="mention"))
    step("chan_desktop")(lambda: props(u2, u2_id, "co", desktop="mention", push="none", email="false"))
    step("chan_ignore_mentions")(lambda: props(u2, u2_id, "co", ignore_channel_mentions="on"))
    step("chan_threads_props")(lambda: props(u2, u2_id, "co", desktop_threads="all", push_threads="mention",
                                             channel_auto_follow_threads="on"))
    # Go stores invalid props unchecked, and then fails every UpdateMember of that member (IsValid): fixed in Rust.
    step("chan_props_bad")(lambda: props(u3, u3_id, "co", mark_unread="sometimes"))
    step("chan_props_bad_then_roles")(lambda: call(u1, "PUT", f"/channels/{ctx['co']}/members/{u3_id}/schemeRoles",
                                                   {"scheme_user": True, "scheme_admin": False}))
    step("chan_props_bad_desktop")(lambda: props(u3, u3_id, "co", desktop="loud"))
    step("chan_props_bad_email")(lambda: props(u3, u3_id, "co", email="yes"))
    step("chan_props_fix")(lambda: props(u3, u3_id, "co", mark_unread="all"))
    step("chan_props_other")(lambda: props(u1, u2_id, "co", desktop="all"))
    step("chan_props_read")(lambda: val(call(u2, "GET", f"/channels/{ctx['co']}/members/me"), np))
    step("chan_unmute")(lambda: props(u2, u2_id, "co", mark_unread="all", desktop="default", push="default",
                                      email="default", ignore_channel_mentions="default"))
    step("chan_props_read2")(lambda: val(call(u2, "GET", f"/channels/{ctx['co']}/members/me"), np))

    # ---- scheme roles ----
    def scheme(token, uid, k, admin_=True):
        return call(token, "PUT", f"/channels/{ctx[k]}/members/{uid}/schemeRoles", {"scheme_user": True, "scheme_admin": admin_})

    step("chan_role_admin")(lambda: scheme(u1, u2_id, "co"))
    step("chan_role_read")(lambda: val(call(u1, "GET", f"/channels/{ctx['co']}/members/{u2_id}"), roles))
    step("chan_role_by_member")(lambda: scheme(u3, u3_id, "co"))
    step("chan_patch_by_chadmin")(lambda: val(call(u2, "PUT", f"/channels/{ctx['co']}/patch", {"header": "by u2"}), chan))
    step("chan_role_bad")(lambda: call(u1, "PUT", f"/channels/{ctx['co']}/members/{u2_id}/schemeRoles", {"scheme_user": False, "scheme_admin": True}))
    step("chan_role_roles")(lambda: call(u1, "PUT", f"/channels/{ctx['co']}/members/{u2_id}/roles", {"roles": "channel_user"}))
    step("chan_role_read2")(lambda: val(call(u1, "GET", f"/channels/{ctx['co']}/members/{u2_id}"), roles))

    # ---- privacy ----
    step("chan_to_private_member")(lambda: call(u3, "PUT", f"/channels/{ctx['co']}/privacy", {"privacy": "P"}))
    step("chan_to_private")(lambda: val(call(u1, "PUT", f"/channels/{ctx['co']}/privacy", {"privacy": "P"}), chan))
    step("chan_to_private_again")(lambda: val(call(u1, "PUT", f"/channels/{ctx['co']}/privacy", {"privacy": "P"}), chan))
    step("chan_to_public")(lambda: val(call(u1, "PUT", f"/channels/{ctx['co']}/privacy", {"privacy": "O"}), chan))
    step("chan_to_public_admin")(lambda: val(call(admin, "PUT", f"/channels/{ctx['co']}/privacy", {"privacy": "O"}), chan))
    step("chan_privacy_bad")(lambda: call(admin, "PUT", f"/channels/{ctx['co']}/privacy", {"privacy": "D"}))
    step("chan_privacy_town")(lambda: call(admin, "PUT", f"/channels/{town}/privacy", {"privacy": "P"}))

    # ---- private channel: members, leave as last ----
    step("chan_priv_add")(lambda: val(call(u1, "POST", f"/channels/{ctx['cp']}/members", {"user_ids": [u2_id, u3_id]}), len))
    step("chan_priv_join_self")(lambda: call(s.spare, "POST", f"/channels/{ctx['cp']}/members", {"user_id": s.spare_id}))
    step("chan_priv_leave_u2")(lambda: call(u2, "DELETE", f"/channels/{ctx['cp']}/members/{u2_id}"))
    step("chan_priv_leave_u3")(lambda: call(u3, "DELETE", f"/channels/{ctx['cp']}/members/{u3_id}"))
    step("chan_priv_leave_last")(lambda: call(u1, "DELETE", f"/channels/{ctx['cp']}/members/{u1_id}"))
    step("chan_priv_after_last")(lambda: val(call(admin, "GET", f"/channels/{ctx['cp']}"), chan))
    step("chan_priv_stats_after")(lambda: val(call(admin, "GET", f"/channels/{ctx['cp']}/stats"), stats))
    step("chan_leave_public")(lambda: call(u1, "DELETE", f"/channels/{s.public}/members/{u1_id}"))
    step("chan_leave_town_square")(lambda: call(u1, "DELETE", f"/channels/{town}/members/{u1_id}"))

    # ---- lists, search, autocomplete ----
    step("chan_private_list_admin")(lambda: val(call(admin, "GET", f"/teams/{team}/channels/private?per_page=200"), chans))
    step("chan_private_list_user")(lambda: call(u1, "GET", f"/teams/{team}/channels/private"))
    step("chan_public_list")(lambda: val(call(u1, "GET", f"/teams/{team}/channels?page=0&per_page=100"), names))
    step("chan_all_admin")(lambda: val(call(admin, "GET", "/channels?per_page=200&exclude_default_channels=true"), names))
    step("chan_search_team")(lambda: val(call(u1, "POST", f"/teams/{team}/channels/search", {"term": "chan"}), chans))
    step("chan_search_team_display")(lambda: val(call(u1, "POST", f"/teams/{team}/channels/search", {"term": "Open 2"}), chans))
    step("chan_search_private_admin")(lambda: val(call(admin, "POST", f"/teams/{team}/channels/search", {"term": "chan"}), chans))
    step("chan_search_all_admin")(lambda: val(call(admin, "POST", "/channels/search", {"term": "chan", "per_page": 50, "page": 0}),
                                              lambda v: sorted(c["name"] for c in v.get("channels", v) if c["name"].startswith("chan-"))
                                              if isinstance(v, dict) else chans(v)))
    step("chan_search_all_private")(lambda: val(call(admin, "POST", "/channels/search", {"term": "chan", "private": True}),
                                                lambda v: sorted(c["name"] for c in (v.get("channels") or []) if c["name"].startswith("chan-"))
                                                if isinstance(v, dict) else chans(v)))
    step("chan_search_all_user")(lambda: call(u1, "POST", "/channels/search", {"term": "chan"}))
    step("chan_autocomplete")(lambda: val(call(u1, "GET", f"/teams/{team}/channels/autocomplete?name=chan"), chans))
    step("chan_autocomplete_bench")(lambda: val(call(u1, "GET", f"/teams/{team}/channels/autocomplete?name=bench-0"), names))
    step("chan_search_autocomplete")(lambda: val(call(u1, "GET", f"/teams/{team}/channels/search_autocomplete?name=chan"), chans))
    step("chan_autocomplete_empty")(lambda: val(call(u1, "GET", f"/teams/{team}/channels/autocomplete?name="), len))

    # ---- sidebar categories ----
    step("cat_list")(lambda: val(call(u1, "GET", cats_url), cats))
    step("cat_create")(lambda: val(keep("cat", call(u1, "POST", cats_url, {
        "user_id": u1_id, "team_id": team, "display_name": "Work", "channel_ids": [ctx["co"]]})), cat))
    step("cat_create_bad_type")(lambda: call(u1, "POST", cats_url, {
        "user_id": u1_id, "team_id": team, "display_name": "Fav", "type": "favorites"}))
    step("cat_create_other")(lambda: call(u2, "POST", cats_url, {"user_id": u1_id, "team_id": team, "display_name": "Nope"}))
    step("cat_get")(lambda: val(call(u1, "GET", f"{cats_url}/{ctx['cat']}"), cat))

    def put_cat(cid=None, kind=None, **change):
        c = call(u1, "GET", f"{cats_url}/{cid}")[1] if cid else cat_of(kind)
        c.update(change)
        return call(u1, "PUT", f"{cats_url}/{c['id']}", c)

    step("cat_rename")(lambda: val(put_cat(ctx["cat"], display_name="Work 2"), cat))
    step("cat_sort_alpha")(lambda: val(put_cat(ctx["cat"], sorting="alpha"), cat))
    step("cat_mute")(lambda: val(put_cat(ctx["cat"], muted=True), cat))
    step("cat_mute_member")(lambda: val(call(u1, "GET", f"/channels/{ctx['co']}/members/me"), lambda v: v["notify_props"]["mark_unread"]))
    step("cat_collapse")(lambda: val(put_cat(ctx["cat"], collapsed=True), cat))
    step("cat_unmute")(lambda: val(put_cat(ctx["cat"], muted=False), cat))
    step("cat_unmute_member")(lambda: val(call(u1, "GET", f"/channels/{ctx['co']}/members/me"), lambda v: v["notify_props"]["mark_unread"]))
    step("cat_rename_channels")(lambda: put_cat(kind="channels", display_name="Renamed"))
    step("cat_move_in")(lambda: val(put_cat(ctx["cat"], channel_ids=[ctx["co"], s.bench00]), cat))
    step("cat_after_move")(lambda: val(call(u1, "GET", cats_url), cats))
    step("cat_sort_manual")(lambda: val(put_cat(ctx["cat"], sorting="manual", channel_ids=[s.bench00, ctx["co"]]),
                                        lambda v: cat(v) | {"first_is_bench00": v["channel_ids"][0] == s.bench00}))
    step("cat_foreign_channel")(lambda: put_cat(ctx["cat"], channel_ids=[ctx["co"], ctx["cp"]]))

    def bulk(**change):
        c = call(u1, "GET", f"{cats_url}/{ctx['cat']}")[1]
        c.update(change)
        return call(u1, "PUT", cats_url, [c])

    step("cat_bulk_update")(lambda: val(bulk(display_name="Bulk", collapsed=False), lambda v: [cat(c) for c in v]))
    step("cat_order")(lambda: val(call(u1, "GET", f"{cats_url}/order"), len))

    def reorder():
        order = call(u1, "GET", f"{cats_url}/order")[1]
        order = [order[-1]] + order[:-1]
        return call(u1, "PUT", f"{cats_url}/order", order)

    step("cat_reorder")(lambda: val(reorder(), len))
    step("cat_reorder_bad")(lambda: call(u1, "PUT", f"{cats_url}/order", ["x" * 26]))
    step("cat_after_reorder")(lambda: val(call(u1, "GET", cats_url), cats))

    # favorites through the category and through the preference
    step("fav_by_category")(lambda: val(put_cat(kind="favorites", channel_ids=[ctx["co"]]), cat))
    step("fav_prefs")(lambda: val(call(u1, "GET", f"/users/{u1_id}/preferences/favorite_channel"),
                                  lambda v: sorted((p["value"], cname.get(p["name"], "bench")) for p in v)))
    step("fav_after_cat")(lambda: val(call(u1, "GET", cats_url), cats))
    step("fav_by_pref")(lambda: call(u1, "PUT", f"/users/{u1_id}/preferences",
                                     [{"user_id": u1_id, "category": "favorite_channel", "name": s.bench00, "value": "true"}]))
    step("fav_after_pref")(lambda: val(call(u1, "GET", cats_url), cats))
    step("fav_unpref")(lambda: call(u1, "PUT", f"/users/{u1_id}/preferences",
                                    [{"user_id": u1_id, "category": "favorite_channel", "name": s.bench00, "value": "false"}]))
    step("fav_after_unpref")(lambda: val(call(u1, "GET", cats_url), cats))
    step("fav_pref_delete")(lambda: call(u1, "POST", f"/users/{u1_id}/preferences/delete",
                                         [{"user_id": u1_id, "category": "favorite_channel", "name": ctx["co"], "value": "true"}]))
    step("fav_after_delete")(lambda: val(call(u1, "GET", cats_url), cats))
    step("cat_delete_favorites")(lambda: call(u1, "DELETE", f"{cats_url}/{cat_of('favorites')['id']}"))

    # where new channels and DMs land
    step("land_channel")(lambda: keepc("cn", call(u1, "POST", "/channels", {
        "team_id": team, "name": "chan-new", "display_name": "Chan New", "type": "O"})))
    step("land_dm")(lambda: keepc("dmx", call(u1, "POST", "/channels/direct", [u1_id, many[5]])))
    step("land_gm")(lambda: keepc("gmx", call(u1, "POST", "/channels/group", [u1_id, many[5], many[6]])))
    step("land_joined")(lambda: call(u1, "POST", f"/channels/{ctx['cn']}/members", {"user_id": u2_id}))
    step("land_u2_cats")(lambda: val(call(u2, "GET", f"/users/{u2_id}/teams/{team}/channels/categories"),
                                     lambda v: [(c["type"], sorted(n for n in (cname.get(i) for i in c["channel_ids"]) if n))
                                                for c in v["categories"]]))
    step("land_cats")(lambda: val(call(u1, "GET", cats_url), cats))
    step("cat_delete")(lambda: call(u1, "DELETE", f"{cats_url}/{ctx['cat']}"))
    step("cat_after_delete")(lambda: val(call(u1, "GET", cats_url), cats))

    # ---- move between teams ----
    step("team2_create")(lambda: keep("t2", call(admin, "POST", "/teams", {"name": "chan-team2", "display_name": "Chan Team 2", "type": "O"})))
    step("team2_add_u1")(lambda: call(admin, "POST", f"/teams/{ctx['t2']}/members", {"team_id": ctx["t2"], "user_id": u1_id}))
    step("move_by_user")(lambda: call(u1, "POST", f"/channels/{ctx['cn']}/move", {"team_id": ctx["t2"], "force": False}))
    step("move_no_force")(lambda: call(admin, "POST", f"/channels/{ctx['cn']}/move", {"team_id": ctx["t2"], "force": False}))
    step("move_force")(lambda: val(call(admin, "POST", f"/channels/{ctx['cn']}/move", {"team_id": ctx["t2"], "force": True}), chan))
    step("move_bad_force")(lambda: call(admin, "POST", f"/channels/{ctx['cn']}/move", {"team_id": ctx["t2"]}))
    step("move_dm")(lambda: call(admin, "POST", f"/channels/{ctx['dmx']}/move", {"team_id": ctx["t2"], "force": True}))
    step("move_after_cats")(lambda: val(call(u1, "GET", f"/users/{u1_id}/teams/{ctx['t2']}/channels/categories"),
                                        lambda v: [(c["type"], sorted(n for n in (cname.get(i) for i in c["channel_ids"]) if n))
                                                   for c in v["categories"]]))
    step("move_after_u2")(lambda: call(u2, "GET", f"/channels/{ctx['cn']}/members/me"))

    # ---- archive, restore ----
    step("hooks_create_ch")(lambda: keepc("ch2", call(u1, "POST", "/channels", {
        "team_id": team, "name": "chan-hooks", "display_name": "Chan Hooks", "type": "O"})))
    step("hooks_in")(lambda: keep("hin", call(admin, "POST", "/hooks/incoming", {"channel_id": ctx["ch2"], "display_name": "in"})))
    step("hooks_out")(lambda: keep("hout", call(admin, "POST", "/hooks/outgoing", {
        "team_id": team, "channel_id": ctx["ch2"], "display_name": "out", "trigger_words": ["zzz"],
        "callback_urls": ["http://localhost:9/none"]})))
    step("archive_hooks")(lambda: call(u1, "DELETE", f"/channels/{ctx['ch2']}"))
    step("archive_hooks_in")(lambda: call(admin, "GET", f"/hooks/incoming/{ctx.get('hin', 'x' * 26)}"))
    step("archive_hooks_out")(lambda: call(admin, "GET", f"/hooks/outgoing/{ctx.get('hout', 'x' * 26)}"))
    step("archive_hooks_list")(lambda: val(call(admin, "GET", f"/hooks/incoming?team_id={team}&per_page=200"),
                                           lambda v: sorted(h["display_name"] for h in v if h["channel_id"] == ctx["ch2"])))
    step("archive_plain")(lambda: call(u1, "DELETE", f"/channels/{ctx['co']}"))
    step("archive_again")(lambda: call(u1, "DELETE", f"/channels/{ctx['co']}"))
    step("archive_patch")(lambda: call(u1, "PUT", f"/channels/{ctx['co']}/patch", {"header": "archived"}))
    step("archive_add_member")(lambda: call(u1, "POST", f"/channels/{ctx['co']}/members", {"user_id": s.spare_id}))
    step("archive_post")(lambda: call(u1, "POST", "/posts", {"channel_id": ctx["co"], "message": "into archive"}))
    step("archive_town")(lambda: call(admin, "DELETE", f"/channels/{town}"))
    step("archived_list")(lambda: val(call(u1, "GET", f"/teams/{team}/channels/deleted?per_page=100"), chans))
    step("archived_list_u3")(lambda: val(call(s.spare, "GET", f"/teams/{team}/channels/deleted?per_page=100"), chans))
    step("archived_list_admin")(lambda: val(call(admin, "GET", f"/teams/{team}/channels/deleted?per_page=100"), chans))
    step("archived_search")(lambda: val(call(u1, "POST", f"/teams/{team}/channels/search_archived", {"term": "chan"}), chans))
    step("archived_cats")(lambda: val(call(u1, "GET", cats_url), cats))
    step("archived_my_channels")(lambda: val(call(u1, "GET", f"/users/me/teams/{team}/channels?include_deleted=true"), chans))
    step("archived_my_channels_no")(lambda: val(call(u1, "GET", f"/users/me/teams/{team}/channels"), chans))
    step("restore_by_user")(lambda: call(u1, "POST", f"/channels/{ctx['co']}/restore"))
    step("restore")(lambda: val(call(admin, "POST", f"/channels/{ctx['co']}/restore"), chan))
    step("restore_again")(lambda: call(admin, "POST", f"/channels/{ctx['co']}/restore"))
    step("restore_hooks")(lambda: val(call(admin, "POST", f"/channels/{ctx['ch2']}/restore"), chan))
    step("restore_hooks_in")(lambda: call(admin, "GET", f"/hooks/incoming/{ctx.get('hin', 'x' * 26)}"))
    step("restore_cats")(lambda: val(call(u1, "GET", cats_url), cats))
    step("restore_stats")(lambda: val(call(u1, "GET", f"/channels/{ctx['co']}/stats"), stats))
