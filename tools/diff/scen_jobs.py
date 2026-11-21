"""Jobs, bulk export and import, and the system console's data: analytics, audits, logs, server status, config
round trips and cache invalidation. Jobs run in the background: each step that makes one polls it until it
finishes (or a timeout), so the job's writes land in the step that made it. Exports are compared by what they hold
(JSONL line types and counts, the imported team's lines with ids and times blanked), not byte for byte."""

import io
import json
import time
import zipfile

DONE = ("success", "error", "canceled", "warning")


def jsonl(lines):
    return ("\n".join(json.dumps(x) for x in lines) + "\n").encode()


def zipped(files):
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in files.items():
            z.writestr(name, data)
    return buf.getvalue()


T0 = 1700000000000


def import_lines():
    """A team, two channels, three existing bench users joined to them with preferences, posts with replies,
    reactions and a flag, a DM and a group message. No new users (Go 12.1 refuses more than 250)."""
    def user(n, extra=None):
        u = {"username": f"bench010{n}", "email": f"bench010{n}@example.com",
             "teams": [{"name": "sqlimp", "roles": "team_user team_admin" if n == 1 else "team_user",
                        "channels": [{"name": "imp-one", "roles": "channel_user", "favorite": n == 1,
                                      "notify_props": {"desktop": "mention", "mobile": "all", "mark_unread": "all"}},
                                     {"name": "imp-two", "roles": "channel_user channel_admin" if n == 2 else "channel_user"}]}]}
        u.update(extra or {})
        return {"type": "user", "user": u}
    return [
        {"type": "version", "version": 1},
        {"type": "team", "team": {"name": "sqlimp", "display_name": "SQL import", "type": "O",
                                  "description": "imported", "allow_open_invite": True}},
        {"type": "channel", "channel": {"team": "sqlimp", "name": "imp-one", "display_name": "Imp one", "type": "O",
                                        "header": "the header", "purpose": "the purpose"}},
        {"type": "channel", "channel": {"team": "sqlimp", "name": "imp-two", "display_name": "Imp two", "type": "P"}},
        user(1, {"use_military_time": "true", "collapse_previews": "true", "message_display": "compact",
                 "tutorial_step": "3", "email_interval": "hour", "show_unread_section": "true"}),
        user(2, {"nickname": "imported nick", "position": "importer"}),
        user(3),
        {"type": "post", "post": {"team": "sqlimp", "channel": "imp-one", "user": "bench0101", "message": "hello import",
                                  "create_at": T0, "flagged_by": ["bench0102"], "is_pinned": True,
                                  "reactions": [{"user": "bench0102", "emoji_name": "smile", "create_at": T0 + 10},
                                                {"user": "bench0103", "emoji_name": "+1", "create_at": T0 + 11}],
                                  "replies": [{"user": "bench0102", "message": "a reply", "create_at": T0 + 20},
                                              {"user": "bench0103", "message": "another reply @bench0101",
                                               "create_at": T0 + 30,
                                               "reactions": [{"user": "bench0101", "emoji_name": "heart",
                                                              "create_at": T0 + 31}]}]}},
        {"type": "post", "post": {"team": "sqlimp", "channel": "imp-one", "user": "bench0102", "message": "second",
                                  "create_at": T0 + 100, "props": {"from_import": "yes"}}},
        {"type": "post", "post": {"team": "sqlimp", "channel": "imp-two", "user": "bench0103", "message": "private one",
                                  "create_at": T0 + 200}},
        {"type": "direct_channel", "direct_channel": {"members": ["bench0101", "bench0102"],
                                                      "favorited_by": ["bench0101"], "header": "dm header"}},
        {"type": "direct_channel", "direct_channel": {"members": ["bench0101", "bench0102", "bench0103"]}},
        {"type": "direct_post", "direct_post": {"channel_members": ["bench0101", "bench0102"], "user": "bench0101",
                                                "message": "dm import", "create_at": T0 + 300,
                                                "reactions": [{"user": "bench0102", "emoji_name": "smile",
                                                               "create_at": T0 + 301}]}},
        {"type": "direct_post", "direct_post": {"channel_members": ["bench0101", "bench0102", "bench0103"],
                                                "user": "bench0103", "message": "gm import", "create_at": T0 + 400,
                                                "flagged_by": ["bench0101"]}},
    ]


def register(s):
    step, call, keep, ctx, body = s.step, s.call, s.keep, s.ctx, s.body
    u1, u1_id, admin = s.u1, s.u1_id, s.admin
    X = "x" * 26

    def c(k):
        return ctx.get(k, X)

    def D(tok, method, path, data=None, unordered=()):
        return body(call(tok, method, path, data), unordered)

    def wait(job_id, timeout=180):
        t0 = time.time()
        st, v, h = call(admin, "GET", f"/jobs/{job_id}")
        while st == 200 and v.get("status") not in DONE and time.time() - t0 < timeout:
            time.sleep(0.3)
            st, v, h = call(admin, "GET", f"/jobs/{job_id}")
        return st, v, h

    def job_view(r):
        """A finished job as compared: its status, type, progress and data, ids and times blanked."""
        st, v, h = r
        if st != 200 or not isinstance(v, dict):
            return r
        d = dict(v.get("data") or {})
        for k in ("export_file", "import_file", "line_number", "error", "export_dir", "is_downloadable"):
            if k in d and k != "error":
                d[k] = "<set>"
        return body((st, {"type": v.get("type"), "status": v.get("status"), "progress": v.get("progress"),
                          "data": d}, h))

    def run_job(key, data, timeout=180, tok=None):
        def go():
            r = call(tok or admin, "POST", "/jobs", data)
            st, v, _ = r
            if st != 201:
                return body(r)
            ctx[key] = v["id"]
            return job_view(wait(v["id"], timeout))
        return go

    # ---- jobs: listing, types, permissions ----
    step("jobs_list")(lambda: D(admin, "GET", "/jobs?page=0&per_page=20", unordered=("",)))
    for t in ("export_process", "import_process", "export_delete", "import_delete", "data_retention", "migrations",
              "active_users", "ldap_sync", "bogus"):
        step(f"jobs_type_{t}")(lambda t=t: D(admin, "GET", f"/jobs/type/{t}?page=0&per_page=20", unordered=("",)))
    step("jobs_type_multi")(lambda: D(admin, "GET", "/jobs?job_type=migrations&job_type=export_process", unordered=("",)))
    step("jobs_list_status")(lambda: D(admin, "GET", "/jobs?status=success&page=0&per_page=50", unordered=("",)))
    step("jobs_list_user")(lambda: D(u1, "GET", "/jobs"))
    step("jobs_type_user")(lambda: D(u1, "GET", "/jobs/type/export_process"))
    step("jobs_get_missing")(lambda: D(admin, "GET", "/jobs/" + X))
    step("jobs_get_badid")(lambda: D(admin, "GET", "/jobs/nope"))
    step("jobs_create_bogus")(lambda: D(admin, "POST", "/jobs", {"type": "bogus"}))
    step("jobs_create_empty")(lambda: D(admin, "POST", "/jobs", {}))
    step("jobs_create_badjson")(lambda: body(call(admin, "POST", "/jobs", raw=b"{", ctype="application/json")))
    step("jobs_create_user")(lambda: D(u1, "POST", "/jobs", {"type": "export_process"}))
    step("jobs_create_ldap")(lambda: D(admin, "POST", "/jobs", {"type": "ldap_sync"}))
    step("jobs_create_es")(lambda: D(admin, "POST", "/jobs", {"type": "elasticsearch_post_indexing"}))
    step("jobs_create_msgexport")(lambda: D(admin, "POST", "/jobs", {"type": "message_export"}))

    # data retention has no worker without a license: the job stays pending and can be cancelled
    def retention():
        r = call(admin, "POST", "/jobs", {"type": "data_retention"})
        if r[0] == 201:
            ctx["dr"] = r[1]["id"]
            time.sleep(2)
            return job_view(call(admin, "GET", f"/jobs/{ctx['dr']}"))
        return body(r)
    step("jobs_retention_create")(retention)
    step("jobs_retention_cancel_user")(lambda: D(u1, "POST", f"/jobs/{c('dr')}/cancel"))
    step("jobs_retention_cancel")(lambda: D(admin, "POST", f"/jobs/{c('dr')}/cancel"))
    step("jobs_retention_after")(lambda: job_view(call(admin, "GET", f"/jobs/{c('dr')}")))
    step("jobs_retention_status")(lambda: D(admin, "PATCH", f"/jobs/{c('dr')}/status", {"status": "pending"}))
    step("jobs_retention_status_force")(lambda: D(admin, "PATCH", f"/jobs/{c('dr')}/status", {"status": "pending", "force": True}))
    step("jobs_retention_status_cancel")(lambda: D(admin, "PATCH", f"/jobs/{c('dr')}/status", {"status": "canceled"}))
    step("jobs_retention_status_bad")(lambda: D(admin, "PATCH", f"/jobs/{c('dr')}/status", {"status": "success"}))
    step("jobs_retention_final")(lambda: job_view(call(admin, "GET", f"/jobs/{c('dr')}")))
    step("jobs_cancel_missing")(lambda: D(admin, "POST", f"/jobs/{X}/cancel"))
    step("jobs_download")(lambda: D(admin, "GET", f"/jobs/{c('dr')}/download"))

    step("jobs_migrations")(run_job("mig", {"type": "migrations"}))
    step("jobs_active_users")(run_job("au", {"type": "active_users"}))
    step("jobs_import_delete")(run_job("idel", {"type": "import_delete"}))
    step("jobs_export_delete")(run_job("edel", {"type": "export_delete"}))
    step("jobs_import_nofile")(run_job("inf", {"type": "import_process"}))
    step("jobs_import_missing")(run_job("imiss", {"type": "import_process", "data": {"import_file": "nope.zip"}}))

    # ---- bulk export of one channel ----
    def export_view(name_key):
        def go():
            st, v, h = call(admin, "GET", "/exports")
            names = [n for n in (v or []) if isinstance(n, str) and n.startswith(c(name_key))]
            if st != 200 or not names:
                return body((st, {"exports": v, "found": names}, h))
            from sqldiff import BASE
            import urllib.request
            req = urllib.request.Request(BASE + "/api/v4/exports/" + names[0])
            req.add_header("Authorization", "Bearer " + admin)
            with urllib.request.urlopen(req, timeout=60) as r:
                raw, status = r.read(), r.status
            out = {"files": [], "types": {}, "lines": {}}
            with zipfile.ZipFile(io.BytesIO(raw)) as z:
                for n in z.namelist():
                    out["files"].append(n.split("/")[-1] if n.endswith(".jsonl") else n)
                    if not n.endswith(".jsonl"):
                        continue
                    for line in z.read(n).decode().splitlines():
                        if not line.strip():
                            continue
                        x = json.loads(line)
                        t = x.get("type")
                        out["types"][t] = out["types"].get(t, 0) + 1
                        # Orders that mean nothing: replies by Posts.Id (random ids), a user's teams and channels
                        # read without ORDER BY.
                        p = x.get("post") or x.get("direct_post") or {}
                        if p.get("replies"):
                            p["replies"].sort(key=lambda r: r.get("create_at", 0))
                        for tm in (x.get("user") or {}).get("teams") or []:
                            (tm.get("channels") or []).sort(key=lambda ch: ch.get("name", ""))
                        if (x.get("user") or {}).get("teams"):
                            x["user"]["teams"].sort(key=lambda tm: tm.get("name", ""))
                        # the lines that hold the imported data, by value
                        txt = json.dumps(x, sort_keys=True)
                        if "sqlimp" in txt or "import" in txt and t in ("post", "direct_post", "direct_channel"):
                            out["lines"].setdefault(t, []).append(x)
                        if t == "version":
                            if isinstance((x.get("info") or {}).get("created"), str):
                                x["info"]["created"] = "<time>"
                            out["lines"]["version"] = [x]
            for t in out["lines"]:
                out["lines"][t].sort(key=lambda x: json.dumps(x, sort_keys=True))
            ctx[name_key + "_file"] = names[0]
            return body((status, out, h))
        return go

    step("export_create_user")(lambda: D(u1, "POST", "/jobs", {"type": "export_process"}))
    step("export_channel")(run_job("exp", {"type": "export_process", "data": {
        "team_name": "bench", "channel_name": "bench-00", "include_attachments": "false"}}, timeout=300))
    step("export_list_user")(lambda: D(u1, "GET", "/exports"))
    step("export_read")(export_view("exp"))
    step("export_download_user")(lambda: D(u1, "GET", f"/exports/{c('exp_file')}"))
    step("export_download_missing")(lambda: D(admin, "GET", "/exports/nope.zip"))
    step("export_presign")(lambda: D(admin, "POST", f"/exports/{c('exp_file')}/presign-url"))
    step("export_delete")(lambda: D(admin, "DELETE", f"/exports/{c('exp_file')}"))
    step("export_list_after")(lambda: D(admin, "GET", "/exports"))

    # ---- bulk import ----
    def upload(key, filename, data):
        def go():
            r = call(admin, "POST", "/uploads", {"type": "import", "filename": filename, "file_size": len(data)})
            if r[0] != 201:
                return body(r)
            ctx[key] = r[1]["id"]
            r = call(admin, "POST", f"/uploads/{ctx[key]}", raw=data, ctype="application/octet-stream")
            ctx[key + "_name"] = f"{ctx[key]}_{filename}"
            return body(r)
        return go

    good = zipped({"import.jsonl": jsonl(import_lines())})
    bad = zipped({"import.jsonl": jsonl([{"type": "version", "version": 1},
                                         {"type": "team", "team": {"name": "sqlbad", "display_name": "Bad", "type": "X"}}])})
    nojsonl = zipped({"readme.txt": b"no jsonl here"})
    step("import_upload_user")(lambda: D(u1, "POST", "/uploads", {"type": "import", "filename": "x.zip", "file_size": 10}))
    step("import_upload_create")(lambda: body(keep("up_probe", call(admin, "POST", "/uploads",
                                                                   {"type": "import", "filename": "probe.zip", "file_size": 10}))))
    step("import_upload_get")(lambda: D(admin, "GET", f"/uploads/{c('up_probe')}"))
    step("import_upload")(upload("up", "sqldiff.zip", good))
    step("import_upload_bad")(upload("upbad", "bad.zip", bad))
    step("import_upload_nojsonl")(upload("upno", "nojsonl.zip", nojsonl))
    step("import_list")(lambda: D(admin, "GET", "/imports", unordered=("",)))
    step("import_list_user")(lambda: D(u1, "GET", "/imports"))
    # the "dry run": a file that fails validation writes nothing
    step("import_run_bad")(lambda: run_job("ibad", {"type": "import_process", "data": {"import_file": c("upbad_name")}})())
    step("import_run_nojsonl")(lambda: run_job("ino", {"type": "import_process", "data": {"import_file": c("upno_name")}})())
    step("import_run_bad_team")(lambda: D(admin, "GET", "/teams/name/sqlbad"))
    step("import_run")(lambda: run_job("imp", {"type": "import_process", "data": {"import_file": c("up_name")}})())

    # ---- what the import made ----
    def keep_team():
        r = call(admin, "GET", "/teams/name/sqlimp")
        if r[0] == 200:
            ctx["it"] = r[1]["id"]
        return body(r)
    step("import_read_team")(keep_team)

    def chan(name, key):
        def go():
            r = call(admin, "GET", f"/teams/{c('it')}/channels/name/{name}?include_deleted=true")
            if r[0] == 200:
                ctx[key] = r[1]["id"]
            return body(r)
        return go
    step("import_read_chan_one")(chan("imp-one", "ic1"))
    step("import_read_chan_two")(chan("imp-two", "ic2"))
    step("import_read_members")(lambda: D(admin, "GET", f"/teams/{c('it')}/members", unordered=("",)))
    step("import_read_chan_members")(lambda: D(admin, "GET", f"/channels/{c('ic1')}/members", unordered=("",)))
    step("import_read_chan2_members")(lambda: D(admin, "GET", f"/channels/{c('ic2')}/members", unordered=("",)))
    step("import_read_posts")(lambda: D(u1, "GET", f"/channels/{c('ic1')}/posts"))
    step("import_read_posts2")(lambda: D(admin, "GET", f"/channels/{c('ic2')}/posts"))
    step("import_read_pinned")(lambda: D(u1, "GET", f"/channels/{c('ic1')}/pinned"))
    step("import_read_stats")(lambda: D(admin, "GET", f"/channels/{c('ic1')}/stats"))
    step("import_read_prefs")(lambda: D(u1, "GET", f"/users/{u1_id}/preferences", unordered=("",)))
    step("import_read_u2")(lambda: D(admin, "GET", f"/users/{s.u2_id}"))
    step("import_read_u1_teams")(lambda: D(u1, "GET", f"/users/{u1_id}/teams/members", unordered=("",)))
    step("import_read_unread")(lambda: D(u1, "GET", f"/users/{u1_id}/teams/{c('it')}/channels/members", unordered=("",)))

    def dm_posts():
        r = call(u1, "POST", "/channels/direct", [u1_id, s.u2_id])
        if r[0] not in (200, 201):
            return body(r)
        return D(u1, "GET", f"/channels/{r[1]['id']}/posts")
    step("import_read_dm")(dm_posts)
    step("import_read_flagged")(lambda: D(s.u2, "GET", f"/users/{s.u2_id}/posts/flagged"))
    # a second run of the same file: the import is idempotent
    step("import_run_again")(lambda: run_job("imp2", {"type": "import_process", "data": {"import_file": c("up_name")}})())
    step("import_read_posts_again")(lambda: D(u1, "GET", f"/channels/{c('ic1')}/posts"))
    step("import_export_team")(lambda: run_job("exp2", {"type": "export_process", "data": {
        "team_name": "sqlimp", "include_attachments": "false"}}, timeout=300)())
    step("import_export_read")(export_view("exp2"))
    step("import_delete_user")(lambda: D(u1, "DELETE", f"/imports/{c('upbad_name')}"))
    step("import_delete")(lambda: D(admin, "DELETE", f"/imports/{c('upbad_name')}"))
    step("import_delete_missing")(lambda: D(admin, "DELETE", "/imports/nope.zip"))
    step("import_list_after")(lambda: D(admin, "GET", "/imports", unordered=("",)))
    step("import_jobs_list")(lambda: D(admin, "GET", "/jobs/type/import_process", unordered=("",)))

    # ---- analytics and system data ----
    for name in ("standard", "post_counts_day", "user_counts_with_posts_day", "extra_counts", "bogus"):
        step(f"analytics_{name}")(lambda n=name: D(admin, "GET", f"/analytics/old?name={n}"))
        step(f"analytics_{name}_team")(lambda n=name: D(admin, "GET", f"/analytics/old?name={n}&team_id={c('it')}"))
    step("analytics_user")(lambda: D(u1, "GET", "/analytics/old?name=standard"))
    step("analytics_default")(lambda: D(admin, "GET", "/analytics/old"))
    step("users_stats")(lambda: D(admin, "GET", "/users/stats"))
    step("audits")(lambda: call(admin, "GET", "/audits?page=0&per_page=20"))
    step("audits_user")(lambda: D(u1, "GET", "/audits"))
    step("logs_get")(lambda: call(admin, "GET", "/logs?page=0&logs_per_page=5"))
    step("logs_get_user")(lambda: D(u1, "GET", "/logs"))
    step("logs_post")(lambda: D(u1, "POST", "/logs", {"level": "ERROR", "message": "sqldiff log line"}))
    step("logs_query")(lambda: call(admin, "POST", "/logs/query", {}))
    step("ping")(lambda: call(None, "GET", "/system/ping"))
    step("ping_status_user")(lambda: call(u1, "GET", "/system/ping?get_server_status=true"))
    step("ping_status")(lambda: call(admin, "GET", "/system/ping?get_server_status=true&use_rest_semantics=true"))
    step("ping_status_keys")(lambda: ping_keys())

    def ping_keys():
        st, v, h = call(admin, "GET", "/system/ping?get_server_status=true")
        return body((st, {k: (v[k] if k in ("status", "database_status", "filestore_status", "ActiveSearchBackend",
                                                "CanReceiveNotifications") else type(v[k]).__name__)
                          for k in (v or {})}, h))
    step("schema_version")(lambda: call(admin, "GET", "/system/schema/version"))
    step("server_busy")(lambda: D(admin, "GET", "/server_busy"))

    # ---- config round trip ----
    step("config_get")(lambda: call(admin, "GET", "/config"))
    step("config_get_user")(lambda: D(u1, "GET", "/config"))
    step("config_patch")(lambda: cfg_view(call(admin, "PUT", "/config/patch",
                                                {"TeamSettings": {"SiteName": "SQLDiff site"},
                                                 "ServiceSettings": {"ExtendSessionLengthWithActivity": True}})))

    def cfg_view(r):
        st, v, h = r
        if st != 200 or not isinstance(v, dict):
            return body(r)
        return body((st, {"SiteName": v["TeamSettings"].get("SiteName"),
                          "Extend": v["ServiceSettings"].get("ExtendSessionLengthWithActivity"),
                          "sections": sorted(v)}, h))
    step("config_patch_user")(lambda: D(u1, "PUT", "/config/patch", {"TeamSettings": {"SiteName": "x"}}))
    step("config_patch_invalid")(lambda: D(admin, "PUT", "/config/patch", {"TeamSettings": {"MaxUsersPerTeam": 0}}))
    step("config_client")(lambda: client_site())

    def client_site():
        st, v, h = call(u1, "GET", "/config/client?format=old")
        return body((st, {"SiteName": (v or {}).get("SiteName")}, h))
    step("config_reload")(lambda: D(admin, "POST", "/config/reload"))
    step("config_after_reload")(lambda: cfg_view(call(admin, "GET", "/config")))
    step("config_patch_back")(lambda: cfg_view(call(admin, "PUT", "/config/patch", {"TeamSettings": {"SiteName": "Mattermost"},
                                                                                      "ServiceSettings": {"ExtendSessionLengthWithActivity": False}})))
    step("config_reload_user")(lambda: D(u1, "POST", "/config/reload"))
    step("config_environment")(lambda: D(admin, "GET", "/config/environment"))
    step("caches_invalidate")(lambda: D(admin, "POST", "/caches/invalidate"))
    step("caches_invalidate_user")(lambda: D(u1, "POST", "/caches/invalidate"))
    step("caches_after_read")(lambda: D(u1, "GET", f"/channels/{c('ic1')}/posts"))
