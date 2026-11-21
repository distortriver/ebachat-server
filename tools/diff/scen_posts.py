"""Posts in depth for sqldiff.py: threads with collapsed threads on and off, follow / unfollow, the thread list and
its unread filters, mentions in threads (@user, @channel, @here) and the counters they leave (channel members, team
unreads, threads), reactions, pins, edits and their history, deleting a root with replies, reminders, flags, marking
a channel unread, paging around the unread post, drafts in a thread, scheduled posts, priority and acknowledgements.

Steps whose name ends in "?" read counters back: their answer is reduced to "field=value" keys, so the comparison
shows when a count differs, not only when a key does (ids differ between the two runs, so they are left out)."""

import time


def register(s):
    step, call, keep, ctx = s.step, s.call, s.keep, s.ctx
    u1, u1_id, u2, u2_id, u3, u3_id = s.u1, s.u1_id, s.u2, s.u2_id, s.u3, s.u3_id
    admin, team, impl = s.admin, s.team, s.impl

    def probe(r, fields, sub=None):
        """The answer reduced to field=value keys (sub picks a nested object first)."""
        st, v, h = r
        if st >= 300 or not isinstance(v, dict):
            return r
        if sub:
            v = sub(v)
            if v is None:
                return st, {"missing": 1}, h
        return st, {f"{f}={v.get(f)!r}": 1 for f in fields}, h

    def shape(r, f):
        """The answer reduced to what f says about it (a dict of keys)."""
        st, v, h = r
        if st >= 300:
            return r
        return st, {k: 1 for k in f(v)}, h

    def noid(p):
        """A priority with its post and channel ids masked (they differ between the runs)."""
        return p if not isinstance(p, dict) else {k: ("id" if k in ("PostId", "ChannelId") else v) for k, v in p.items()}

    member = ["msg_count", "mention_count", "msg_count_root", "mention_count_root", "urgent_mention_count"]
    thread = ["reply_count", "unread_replies", "unread_mentions", "is_urgent"]
    totals = ["total", "total_unread_threads", "total_unread_mentions", "total_unread_urgent_mentions"]

    def team_unread(tok):
        def pick(v):
            return next((t for t in v if t.get("team_id") == team), None)
        st, v, h = call(tok, "GET", "/users/me/teams/unread")
        if st >= 300:
            return st, v, h
        t = pick(v)
        if t is None:
            return st, {"missing": 1}, h
        return st, {f"{f}={t.get(f)!r}": 1 for f in ["msg_count", "mention_count", "msg_count_root", "mention_count_root",
                                                     "thread_count", "thread_mention_count",
                                                     "thread_urgent_mention_count"]}, h

    def counters(name, tok, uid):
        step(f"{name}_member?")(lambda: probe(call(tok, "GET", f"/channels/{ctx['ch']}/members/me"), member))
        step(f"{name}_team?")(lambda: team_unread(tok))
        step(f"{name}_thread?")(lambda: probe(call(tok, "GET", f"/users/{uid}/teams/{team}/threads/{ctx['root']}"),
                                              thread))
        step(f"{name}_threads?")(lambda: probe(call(tok, "GET", f"/users/{uid}/teams/{team}/threads?per_page=25"),
                                               totals))

    def post(tok, k, message, root="", **extra):
        body = {"channel_id": ctx["ch"], "message": message, "root_id": ctx[root] if root else ""}
        body.update(extra)
        r = call(tok, "POST", "/posts", body)
        if k:
            keep(k, r)
        if r[0] < 300:
            ctx["ts_" + (k or "last")] = r[1]["create_at"]
        return r

    def pref(tok, uid, cat, name, value):
        return call(tok, "PUT", f"/users/{uid}/preferences",
                    [{"user_id": uid, "category": cat, "name": name, "value": value}])

    # ---- setup: collapsed threads up to the user, u1 on, u2 off, u3 on by the default ----
    step("p_cfg_crt")(lambda: call(admin, "PUT", "/config/patch", {"ServiceSettings": {"CollapsedThreads": "default_on"}}))
    step("p_crt_on")(lambda: pref(u1, u1_id, "display_settings", "collapsed_reply_threads", "on"))
    step("p_crt_off")(lambda: pref(u2, u2_id, "display_settings", "collapsed_reply_threads", "off"))
    step("p_channel")(lambda: keep("ch", call(u1, "POST", "/channels", {"team_id": team, "name": f"posts-{impl}",
                                                                         "display_name": "Posts", "type": "O"})))
    step("p_members")(lambda: call(u1, "POST", f"/channels/{ctx['ch']}/members", {"user_ids": [u2_id, u3_id]}))
    step("p_views")(lambda: [call(t, "POST", "/channels/members/me/view", {"channel_id": ctx["ch"]}) for t in (u2, u3)][-1])

    # ---- a root with props and hashtags, replies with mentions ----
    step("p_root")(lambda: post(u1, "root", "root #alpha #beta", props={
        "attachments": [{"fallback": "fb", "color": "#ff0000", "pretext": "pre", "text": "attached", "title": "T",
                         "fields": [{"title": "F", "value": "v", "short": True}]}]}))
    counters("p_root", u2, u2_id)
    step("p_reply1")(lambda: post(u2, "r1", "reply one", "root"))
    counters("p_reply1_u1", u1, u1_id)
    step("p_reply_user")(lambda: post(u3, "r2", "@bench0101 in thread", "root"))
    counters("p_reply_user_u1", u1, u1_id)
    step("p_reply_channel")(lambda: post(u1, "r3", "@bench0102 and @channel", "root"))
    counters("p_reply_channel_u2", u2, u2_id)
    counters("p_reply_channel_u3", u3, u3_id)
    step("p_reply_here")(lambda: post(u2, "r4", "@here look", "root"))
    counters("p_reply_here_u1", u1, u1_id)
    counters("p_reply_here_u3", u3, u3_id)
    step("p_post_root2")(lambda: post(u3, "root2", "second root @bench0102"))
    counters("p_root2_u2", u2, u2_id)

    # ---- reading threads ----
    step("p_thread")(lambda: shape(call(u1, "GET", f"/posts/{ctx['root']}/thread"),
                                   lambda v: [f"order={len(v['order'])}", f"posts={len(v['posts'])}",
                                              f"has_next={v.get('has_next')}"] + sorted(v.keys())))
    step("p_thread_crt")(lambda: shape(call(u1, "GET", f"/posts/{ctx['root']}/thread?collapsedThreads=true&perPage=2&direction=down"),
                                       lambda v: [f"order={len(v['order'])}", f"has_next={v.get('has_next')}",
                                                  f"next={'x' if v.get('next_post_id') else ''}"]))
    step("p_channel_posts_crt")(lambda: shape(call(u1, "GET", f"/channels/{ctx['ch']}/posts?collapsedThreads=true&collapsedThreadsExtended=true"),
                                              lambda v: [f"order={len(v['order'])}"] +
                                              [f"{p['message'][:12]}:{p.get('reply_count')}:{'participants' in p}"
                                               for p in v["posts"].values()]))
    step("p_channel_posts")(lambda: shape(call(u2, "GET", f"/channels/{ctx['ch']}/posts"),
                                          lambda v: [f"order={len(v['order'])}"]))
    step("p_threads_unread?")(lambda: probe(call(u1, "GET", f"/users/{u1_id}/teams/{team}/threads?unread=true&extended=true"), totals))
    step("p_threads_totals?")(lambda: probe(call(u1, "GET", f"/users/{u1_id}/teams/{team}/threads?totalsOnly=true"), totals))
    step("p_threads_only")(lambda: shape(call(u1, "GET", f"/users/{u1_id}/teams/{team}/threads?threadsOnly=true&per_page=5"),
                                         lambda v: sorted(v.keys()) + [f"n={len(v.get('threads') or [])}"]))
    step("p_threads_both")(lambda: call(u1, "GET", f"/users/{u1_id}/teams/{team}/threads?threadsOnly=true&totalsOnly=true"))
    step("p_threads_since")(lambda: shape(call(u1, "GET", f"/users/{u1_id}/teams/{team}/threads?since={ctx['ts_root']}"),
                                          lambda v: [f"n={len(v.get('threads') or [])}"]))
    step("p_thread_extended")(lambda: shape(call(u1, "GET", f"/users/{u1_id}/teams/{team}/threads/{ctx['root']}?extended=true"),
                                            lambda v: sorted(v.keys()) + [f"participants={len(v.get('participants') or [])}",
                                                                          f"ptype={type((v.get('participants') or [None])[0]).__name__}"]))

    # ---- follow / unfollow, read / unread ----
    step("p_unfollow")(lambda: call(u3, "DELETE", f"/users/{u3_id}/teams/{team}/threads/{ctx['root']}/following"))
    counters("p_unfollow_u3", u3, u3_id)
    step("p_follow")(lambda: call(u3, "PUT", f"/users/{u3_id}/teams/{team}/threads/{ctx['root']}/following"))
    counters("p_follow_u3", u3, u3_id)
    step("p_read_thread")(lambda: call(u1, "PUT", f"/users/{u1_id}/teams/{team}/threads/{ctx['root']}/read/{ctx['ts_r4'] + 1}"))
    counters("p_read_thread_u1", u1, u1_id)
    step("p_unread_thread")(lambda: call(u1, "POST", f"/users/{u1_id}/teams/{team}/threads/{ctx['root']}/set_unread/{ctx['r2']}"))
    counters("p_unread_thread_u1", u1, u1_id)
    step("p_read_all")(lambda: call(u1, "PUT", f"/users/{u1_id}/teams/{team}/threads/read"))
    counters("p_read_all_u1", u1, u1_id)
    step("p_read_thread_other")(lambda: call(u2, "PUT", f"/users/{u1_id}/teams/{team}/threads/{ctx['root']}/read/1"))

    # ---- reactions ----
    step("p_react_u2")(lambda: call(u2, "POST", "/reactions", {"user_id": u2_id, "post_id": ctx["root"], "emoji_name": "+1"}))
    step("p_react_u3")(lambda: call(u3, "POST", "/reactions", {"user_id": u3_id, "post_id": ctx["root"], "emoji_name": "heart"}))
    step("p_react_u1")(lambda: call(u1, "POST", "/reactions", {"user_id": u1_id, "post_id": ctx["root"], "emoji_name": "smile"}))
    step("p_react_reply")(lambda: call(u1, "POST", "/reactions", {"user_id": u1_id, "post_id": ctx["r1"], "emoji_name": "thumbsup"}))
    step("p_react_again")(lambda: call(u2, "POST", "/reactions", {"user_id": u2_id, "post_id": ctx["root"], "emoji_name": "+1"}))
    step("p_react_bad")(lambda: call(u2, "POST", "/reactions", {"user_id": u2_id, "post_id": ctx["root"], "emoji_name": "no_such_emoji_x"}))
    step("p_react_other")(lambda: call(u2, "POST", "/reactions", {"user_id": u1_id, "post_id": ctx["root"], "emoji_name": "tada"}))
    step("p_reactions")(lambda: shape(call(u1, "GET", f"/posts/{ctx['root']}/reactions"),
                                      lambda v: sorted(r["emoji_name"] for r in v)))
    step("p_unreact")(lambda: call(u2, "DELETE", f"/users/{u2_id}/posts/{ctx['root']}/reactions/+1"))
    step("p_unreact_none")(lambda: call(u2, "DELETE", f"/users/{u2_id}/posts/{ctx['root']}/reactions/+1"))
    step("p_reactions_bulk")(lambda: call(u1, "POST", "/posts/ids/reactions", [ctx["root"], ctx["r1"]]))
    step("p_root_meta")(lambda: shape(call(u1, "GET", f"/posts/{ctx['root']}"),
                                      lambda v: sorted(v.get("metadata", {}).keys()) +
                                      [f"reactions={len(v.get('metadata', {}).get('reactions') or [])}",
                                       f"has_reactions={v.get('has_reactions')}",
                                       f"hashtags={v.get('hashtags')}"]))

    # ---- pins ----
    step("p_pin")(lambda: call(u2, "POST", f"/posts/{ctx['r1']}/pin"))
    step("p_pin_root")(lambda: call(u1, "POST", f"/posts/{ctx['root']}/pin"))
    step("p_pinned")(lambda: shape(call(u1, "GET", f"/channels/{ctx['ch']}/pinned"), lambda v: [f"order={len(v['order'])}"]))
    step("p_stats?")(lambda: probe(call(u1, "GET", f"/channels/{ctx['ch']}/stats"), ["member_count", "pinnedpost_count", "files_count"]))
    step("p_unpin")(lambda: call(u1, "POST", f"/posts/{ctx['r1']}/unpin"))
    step("p_pinned_after")(lambda: shape(call(u1, "GET", f"/channels/{ctx['ch']}/pinned"), lambda v: [f"order={len(v['order'])}"]))

    # ---- edits ----
    step("p_patch_root")(lambda: call(u1, "PUT", f"/posts/{ctx['root']}/patch", {"message": "root edited #gamma"}))
    step("p_put_reply")(lambda: call(u2, "PUT", f"/posts/{ctx['r1']}", {"id": ctx["r1"], "message": "reply one edited"}))
    step("p_patch_reply_again")(lambda: call(u2, "PUT", f"/posts/{ctx['r1']}/patch", {"message": "reply one edited twice"}))
    step("p_patch_other")(lambda: call(u3, "PUT", f"/posts/{ctx['r1']}/patch", {"message": "not mine"}))
    step("p_patch_props")(lambda: call(u1, "PUT", f"/posts/{ctx['root']}/patch", {"props": {"attachments": []}}))
    step("p_edit_history")(lambda: shape(call(u1, "GET", f"/posts/{ctx['root']}/edit_history"),
                                         lambda v: [f"n={len(v)}"] + [p["message"] for p in v]))
    step("p_edit_history_reply")(lambda: shape(call(u2, "GET", f"/posts/{ctx['r1']}/edit_history"),
                                               lambda v: [f"n={len(v)}"] + [p["message"] for p in v]))
    step("p_edit_history_other")(lambda: call(u1, "GET", f"/posts/{ctx['r1']}/edit_history"))
    step("p_edit_history_none")(lambda: call(u3, "GET", f"/posts/{ctx['r2']}/edit_history"))
    step("p_root_edited")(lambda: shape(call(u1, "GET", f"/posts/{ctx['root']}"),
                                        lambda v: [f"hashtags={v.get('hashtags')}", f"edited={v.get('edit_at', 0) > 0}",
                                                   f"props={sorted(v.get('props', {}).keys())}"]))

    # ---- reminders, flags ----
    step("p_reminder")(lambda: call(u1, "POST", f"/users/{u1_id}/posts/{ctx['r2']}/reminder",
                                    {"target_time": int(time.time()) + 3600}))
    step("p_reminder_past")(lambda: call(u1, "POST", f"/users/{u1_id}/posts/{ctx['r2']}/reminder",
                                         {"target_time": int(time.time()) - 3600}))
    step("p_reminder_other")(lambda: call(u2, "POST", f"/users/{u1_id}/posts/{ctx['r2']}/reminder",
                                          {"target_time": int(time.time()) + 3600}))
    step("p_flag")(lambda: pref(u3, u3_id, "flagged_post", ctx["r1"], "true"))
    step("p_flag_root")(lambda: pref(u3, u3_id, "flagged_post", ctx["root"], "true"))
    step("p_flagged")(lambda: shape(call(u3, "GET", f"/users/{u3_id}/posts/flagged?channel_id={ctx['ch']}"),
                                    lambda v: [f"order={len(v['order'])}"]))
    step("p_flagged_team")(lambda: shape(call(u3, "GET", f"/users/{u3_id}/posts/flagged?team_id={team}"),
                                         lambda v: [f"order={len(v['order'])}"]))
    step("p_unflag")(lambda: call(u3, "POST", f"/users/{u3_id}/preferences/delete",
                                  [{"user_id": u3_id, "category": "flagged_post", "name": ctx["r1"], "value": "true"}]))
    step("p_flagged_after")(lambda: shape(call(u3, "GET", f"/users/{u3_id}/posts/flagged"),
                                          lambda v: [f"order={len(v['order'])}"]))

    # ---- set_unread, paging ----
    step("p_view_u2")(lambda: call(u2, "POST", "/channels/members/me/view", {"channel_id": ctx["ch"]}))
    step("p_set_unread")(lambda: probe(call(u2, "POST", f"/users/{u2_id}/posts/{ctx['root']}/set_unread"),
                                       ["msg_count", "mention_count", "msg_count_root", "mention_count_root",
                                        "urgent_mention_count"]))
    counters("p_set_unread_u2", u2, u2_id)
    step("p_set_unread_crt")(lambda: probe(call(u1, "POST", f"/users/{u1_id}/posts/{ctx['root2']}/set_unread?collapsed_threads_supported=true"),
                                           ["msg_count", "mention_count", "msg_count_root", "mention_count_root"]))
    counters("p_set_unread_u1", u1, u1_id)
    step("p_posts_unread")(lambda: shape(call(u2, "GET", f"/users/{u2_id}/channels/{ctx['ch']}/posts/unread?limit_before=2&limit_after=2"),
                                         lambda v: [f"order={len(v['order'])}", f"next={bool(v.get('next_post_id'))}",
                                                    f"prev={bool(v.get('prev_post_id'))}", f"has_next={v.get('has_next')}"]))
    step("p_posts_unread_crt")(lambda: shape(call(u2, "GET", f"/users/{u2_id}/channels/{ctx['ch']}/posts/unread?limit_before=2&limit_after=2&collapsedThreads=true"),
                                             lambda v: [f"order={len(v['order'])}", f"next={bool(v.get('next_post_id'))}",
                                                        f"prev={bool(v.get('prev_post_id'))}"]))
    step("p_posts_before")(lambda: shape(call(u2, "GET", f"/channels/{ctx['ch']}/posts?before={ctx['r3']}&per_page=2"),
                                         lambda v: [f"order={len(v['order'])}", f"next={bool(v.get('next_post_id'))}",
                                                    f"prev={bool(v.get('prev_post_id'))}"]))
    step("p_posts_after")(lambda: shape(call(u2, "GET", f"/channels/{ctx['ch']}/posts?after={ctx['r1']}&per_page=2"),
                                        lambda v: [f"order={len(v['order'])}", f"next={bool(v.get('next_post_id'))}",
                                                   f"prev={bool(v.get('prev_post_id'))}"]))
    step("p_posts_since")(lambda: shape(call(u2, "GET", f"/channels/{ctx['ch']}/posts?since={ctx['ts_r2']}"),
                                        lambda v: [f"order={len(v['order'])}", f"posts={len(v['posts'])}"]))
    step("p_posts_since_crt")(lambda: shape(call(u1, "GET", f"/channels/{ctx['ch']}/posts?since={ctx['ts_r2']}&collapsedThreads=true"),
                                            lambda v: [f"order={len(v['order'])}", f"posts={len(v['posts'])}"]))
    step("p_view_after")(lambda: call(u2, "POST", "/channels/members/me/view", {"channel_id": ctx["ch"]}))
    counters("p_view_after_u2", u2, u2_id)

    # ---- drafts in a thread ----
    step("p_draft_thread")(lambda: call(u1, "POST", "/drafts", {"user_id": u1_id, "channel_id": ctx["ch"],
                                                                "root_id": ctx["root"], "message": "thread draft"}))
    step("p_draft_channel")(lambda: call(u1, "POST", "/drafts", {"user_id": u1_id, "channel_id": ctx["ch"],
                                                                 "root_id": "", "message": "channel draft"}))
    step("p_drafts")(lambda: shape(call(u1, "GET", f"/users/{u1_id}/teams/{team}/drafts"),
                                   lambda v: sorted(f"{d['message']}:{bool(d['root_id'])}" for d in v)))
    step("p_draft_delete_thread")(lambda: call(u1, "DELETE", f"/users/{u1_id}/channels/{ctx['ch']}/drafts/{ctx['root']}"))
    step("p_drafts_after")(lambda: shape(call(u1, "GET", f"/users/{u1_id}/teams/{team}/drafts"),
                                         lambda v: sorted(f"{d['message']}:{bool(d['root_id'])}" for d in v)))
    step("p_draft_reply_post")(lambda: post(u1, "r5", "posted from a draft", "root"))

    # ---- scheduled posts, priority, acknowledgements ----
    step("p_schedule")(lambda: call(u1, "POST", "/posts/schedule", {"channel_id": ctx["ch"], "message": "later",
                                                                    "scheduled_at": int(time.time() * 1000) + 3600000}))
    step("p_scheduled_list")(lambda: call(u1, "GET", f"/posts/scheduled/team/{team}"))
    step("p_priority")(lambda: shape(post(u1, "prio", "important @bench0102",
                                          metadata={"priority": {"priority": "important"}}),
                                     lambda v: sorted(v.get("metadata", {}).keys()) +
                                     [f"prio={noid(v.get('metadata', {}).get('priority'))}"]))
    step("p_priority_urgent")(lambda: shape(post(u1, "urgent", "urgent @bench0102",
                                                 metadata={"priority": {"priority": "urgent", "requested_ack": True}}),
                                            lambda v: [f"prio={noid(v.get('metadata', {}).get('priority'))}"]))
    step("p_priority_persistent")(lambda: post(u1, None, "persistent @bench0102",
                                               metadata={"priority": {"priority": "urgent", "persistent_notifications": True}}))
    step("p_priority_reply")(lambda: post(u1, None, "prio reply", "root", metadata={"priority": {"priority": "important"}}))
    step("p_priority_bad")(lambda: post(u1, None, "bad prio", metadata={"priority": {"priority": "whatever"}}))
    counters("p_priority_u2", u2, u2_id)
    step("p_ack")(lambda: call(u2, "POST", f"/users/{u2_id}/posts/{ctx.get('urgent', 'x' * 26)}/ack"))
    step("p_unack")(lambda: call(u2, "DELETE", f"/users/{u2_id}/posts/{ctx.get('urgent', 'x' * 26)}/ack"))
    step("p_prio_get")(lambda: shape(call(u2, "GET", f"/posts/{ctx.get('prio', 'x' * 26)}"),
                                     lambda v: [f"prio={noid(v.get('metadata', {}).get('priority'))}"]))

    # ---- deleting: a reply, then a root with replies ----
    step("p_delete_reply")(lambda: call(u2, "DELETE", f"/posts/{ctx['r4']}"))
    counters("p_delete_reply_u1", u1, u1_id)
    step("p_root3")(lambda: post(u2, "root3", "doomed root"))
    step("p_root3_r1")(lambda: post(u3, "root3_r1", "doomed reply @bench0101", "root3"))
    step("p_root3_r2")(lambda: post(u1, "root3_r2", "doomed reply two", "root3"))
    step("p_root3_react")(lambda: call(u1, "POST", "/reactions", {"user_id": u1_id, "post_id": ctx["root3_r1"], "emoji_name": "smile"}))
    step("p_root3_pin")(lambda: call(u2, "POST", f"/posts/{ctx['root3_r1']}/pin"))
    step("p_root3_flag")(lambda: pref(u3, u3_id, "flagged_post", ctx["root3"], "true"))
    step("p_root3_threads?")(lambda: probe(call(u1, "GET", f"/users/{u1_id}/teams/{team}/threads?per_page=25"), totals))
    step("p_delete_root")(lambda: call(u2, "DELETE", f"/posts/{ctx['root3']}"))
    step("p_deleted_thread")(lambda: call(u1, "GET", f"/posts/{ctx['root3']}/thread"))
    step("p_deleted_reply")(lambda: call(u1, "GET", f"/posts/{ctx['root3_r1']}"))
    step("p_deleted_thread_member")(lambda: call(u1, "GET", f"/users/{u1_id}/teams/{team}/threads/{ctx['root3']}"))
    step("p_deleted_threads?")(lambda: probe(call(u1, "GET", f"/users/{u1_id}/teams/{team}/threads?per_page=25"), totals))
    step("p_deleted_threads_deleted?")(lambda: probe(call(u1, "GET", f"/users/{u1_id}/teams/{team}/threads?per_page=25&deleted=true"), totals))
    step("p_deleted_flagged")(lambda: shape(call(u3, "GET", f"/users/{u3_id}/posts/flagged"),
                                            lambda v: [f"order={len(v['order'])}"]))
    step("p_deleted_pinned")(lambda: shape(call(u1, "GET", f"/channels/{ctx['ch']}/pinned"),
                                           lambda v: [f"order={len(v['order'])}"]))
    step("p_deleted_stats?")(lambda: probe(call(u1, "GET", f"/channels/{ctx['ch']}/stats"), ["pinnedpost_count"]))
    counters("p_deleted_u1", u1, u1_id)
    step("p_react_deleted")(lambda: call(u1, "POST", "/reactions", {"user_id": u1_id, "post_id": ctx["root3_r2"], "emoji_name": "smile"}))
    step("p_reply_deleted")(lambda: post(u1, None, "reply to a deleted root", "root3"))

    # ---- back to the bench config ----
    step("p_cfg_back")(lambda: call(admin, "PUT", "/config/patch", {"ServiceSettings": {"CollapsedThreads": "always_on"}}))
