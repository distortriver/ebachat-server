"""Files and images: uploads (multipart, one and several files, and the raw body), file info, the file itself, its
thumbnail, preview and link, posts with files and their metadata, deleting such a post, file search and the channel's
files, custom emoji, the profile image and the team icon.

Answers are recorded with their shape and stable values (ids, timestamps and the server's name masked), and binary
answers by their headers, so the comparison sees more than the top-level keys."""

import json
import re
import struct
import urllib.error
import urllib.request
import zlib

ID = re.compile(r"^[a-z0-9]{26}$")
BOUNDARY = "sqldiffBoundary7MA4YWxkTrZu0gW"


def png(w, h, rgb):
    """A w x h PNG of one colour, made with zlib and struct."""
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xffffffff)
    rows = b"".join(b"\0" + bytes(rgb) * w for _ in range(h))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows, 9)) + chunk(b"IEND", b""))


def multipart(fields=(), files=()):
    """fields: (name, value); files: (field, filename, content type, bytes)."""
    out = []
    for name, value in fields:
        out.append(f'--{BOUNDARY}\r\nContent-Disposition: form-data; name="{name}"\r\n\r\n{value}\r\n'.encode())
    for field, filename, ctype, data in files:
        out.append(f'--{BOUNDARY}\r\nContent-Disposition: form-data; name="{field}"; filename="{filename}"\r\n'
                   f'Content-Type: {ctype}\r\n\r\n'.encode() + data + b"\r\n")
    out.append(f"--{BOUNDARY}--\r\n".encode())
    return b"".join(out), f"multipart/form-data; boundary={BOUNDARY}"


def register(s):
    step, call, keep, ctx = s.step, s.call, s.keep, s.ctx
    u1, u1_id, u2, u2_id, u3 = s.u1, s.u1_id, s.u2, s.u2_id, s.u3
    admin, team, impl, BASE = s.admin, s.team, s.impl, s.base

    def mask(v, key=""):
        if isinstance(v, dict):
            return {("<id>" if ID.match(k) else k): mask(x, k) for k, x in v.items()}
        if isinstance(v, list):
            return [mask(x, key) for x in v]
        if isinstance(v, str):
            if ID.match(v) or key.endswith("_id") or key == "id":
                return "<id>" if v else ""
            if key == "mini_preview" and v:
                return "<jpeg>"  # the JPEG encoders differ, the 16x16 picture does not
            return v.replace(impl, "<impl>")
        if isinstance(v, (int, float)) and not isinstance(v, bool) and (key.endswith("_at") or key.endswith("_update")):
            return ("<time>" if v > 0 else "<-time>") if v else 0
        return v

    def shape(r, pick=lambda v: v):
        """The answer with ids and times masked, as the single key of a dict, so sqldiff records it whole."""
        st, v, h = r
        if st >= 300 or v is None:
            return r
        return st, {json.dumps(mask(pick(v)), sort_keys=True): 1}, h

    def raw(token, path, extra=""):
        """A binary answer: its status and the headers a client reads."""
        req = urllib.request.Request(BASE + "/api/v4" + path + extra, method="GET")
        if token:
            req.add_header("Authorization", "Bearer " + token)
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                body, st, h = r.read(), r.status, r.headers
        except urllib.error.HTTPError as e:
            body, st, h = e.read(), e.code, e.headers
        if st >= 300:
            try:
                return st, json.loads(body), h
            except ValueError:
                return st, None, h
        sig = {k.lower(): (h.get(k) or "").replace(impl, "<impl>") for k in
               ("Content-Type", "Content-Disposition", "Cache-Control", "X-Content-Type-Options",
                "Content-Security-Policy")}
        # Images the server encoded (thumbnails, previews, profile images, icons) differ in bytes between encoders.
        sig["len"] = "<image>" if sig["content-type"].startswith("image/") and len(body) != len(red) else len(body)
        sig["magic"] = body[:4].hex()
        return st, {json.dumps(sig, sort_keys=True): 1}, h

    red, blue = png(64, 48, (200, 30, 30)), png(1200, 900, (30, 30, 200))
    text = b"a small text file about sqldiffword\n"

    def upload(token, fields, files):
        body, ctype = multipart(fields, files)
        return call(token, "POST", "/files", raw=body, ctype=ctype)

    def keep_files(name, r):
        st, v, _ = r
        if st < 300 and v and v.get("file_infos"):
            for i, fi in enumerate(v["file_infos"]):
                ctx[f"{name}{i}"] = fi["id"]
        return shape(r)

    # ---- set up: a channel of u1 with u2 in it ----
    step("files_channel")(lambda: keep("fch", call(u1, "POST", "/channels",
                                                    {"team_id": team, "name": f"sqldiff-files-{impl}", "display_name": "Files", "type": "O"})))
    step("files_channel_add")(lambda: call(u1, "POST", f"/channels/{ctx['fch']}/members", {"user_id": u2_id}))

    # ---- uploads ----
    step("files_upload_png")(lambda: keep_files("png", upload(u1, [("channel_id", ctx["fch"]), ("client_ids", "c1")],
                                                              [("files", "red.png", "image/png", red)])))
    step("files_upload_two")(lambda: keep_files("two", upload(u1, [("channel_id", ctx["fch"]), ("client_ids", "c2"), ("client_ids", "c3")],
                                                              [("files", "blue.png", "image/png", blue),
                                                               ("files", "note.txt", "text/plain", text)])))
    step("files_upload_raw")(lambda: keep_files("raw", call(u1, "POST", f"/files?channel_id={ctx['fch']}&filename=raw.txt",
                                                            raw=text, ctype="text/plain")))
    step("files_upload_no_channel")(lambda: upload(u1, [], [("files", "x.txt", "text/plain", text)]))
    step("files_upload_not_member")(lambda: upload(u3, [("channel_id", ctx["fch"])], [("files", "x.txt", "text/plain", text)]))

    # ---- reading the files ----
    step("files_info")(lambda: shape(call(u1, "GET", f"/files/{ctx['png0']}/info")))
    step("files_info_text")(lambda: shape(call(u1, "GET", f"/files/{ctx['two1']}/info")))
    step("files_get")(lambda: raw(u1, f"/files/{ctx['png0']}"))
    step("files_get_download")(lambda: raw(u1, f"/files/{ctx['png0']}", "?download=1"))
    step("files_get_text")(lambda: raw(u1, f"/files/{ctx['two1']}"))
    step("files_thumbnail")(lambda: raw(u1, f"/files/{ctx['png0']}/thumbnail"))
    step("files_preview")(lambda: raw(u1, f"/files/{ctx['png0']}/preview"))
    step("files_preview_big")(lambda: raw(u1, f"/files/{ctx['two0']}/preview"))
    step("files_thumbnail_text")(lambda: raw(u1, f"/files/{ctx['two1']}/thumbnail"))
    step("files_link")(lambda: call(u1, "GET", f"/files/{ctx['png0']}/link"))
    # Before it is attached, only the uploader reads a file.
    step("files_info_other_unattached")(lambda: call(u2, "GET", f"/files/{ctx['png0']}/info"))

    # ---- posts with files ----
    step("files_post")(lambda: keep("fp", call(u1, "POST", "/posts",
                                               {"channel_id": ctx["fch"], "message": "two files",
                                                "file_ids": [ctx["png0"], ctx["two1"]]})))
    step("files_post_shape")(lambda: shape(call(u1, "GET", f"/posts/{ctx['fp']}"),
                                           lambda v: {"file_ids": len(v.get("file_ids") or []),
                                                      "metadata": v.get("metadata")}))
    step("files_post_two")(lambda: keep("fp2", call(u1, "POST", "/posts",
                                                    {"channel_id": ctx["fch"], "message": "more",
                                                     "file_ids": [ctx["two0"], ctx["raw0"]]})))
    step("files_post_reuse")(lambda: call(u1, "POST", "/posts",
                                          {"channel_id": ctx["fch"], "message": "same file again", "file_ids": [ctx["png0"]]}))
    step("files_post_files_info")(lambda: shape(call(u1, "GET", f"/posts/{ctx['fp']}/files/info")))
    step("files_channel_posts")(lambda: shape(call(u2, "GET", f"/channels/{ctx['fch']}/posts?page=0&per_page=30"),
                                              lambda v: sorted(json.dumps(mask(p.get("metadata")), sort_keys=True)
                                                               for p in v["posts"].values())))
    step("files_info_member")(lambda: shape(call(u2, "GET", f"/files/{ctx['png0']}/info")))
    step("files_get_member")(lambda: raw(u2, f"/files/{ctx['png0']}"))
    step("files_info_not_member")(lambda: call(u3, "GET", f"/files/{ctx['png0']}/info"))
    step("files_get_not_member")(lambda: raw(u3, f"/files/{ctx['png0']}"))

    # ---- search ----
    def search(token, body):
        return shape(call(token, "POST", f"/teams/{team}/files/search", body),
                     lambda v: {k: (sorted(json.dumps(mask(fi), sort_keys=True) for fi in v[k].values())
                                    if k == "file_infos" else len(v[k]) if isinstance(v[k], list) else v[k])
                                for k in v})

    step("files_search_name")(lambda: search(u1, {"terms": "note", "is_or_search": False, "page": 0, "per_page": 20}))
    step("files_search_ext")(lambda: search(u1, {"terms": "ext:png", "is_or_search": False, "page": 0, "per_page": 20}))
    step("files_search_content")(lambda: search(u1, {"terms": "sqldiffword", "page": 0, "per_page": 20}))
    step("files_channel_files")(lambda: search(u2, {"terms": f"channel:{ctx['fch']}", "is_or_search": False,
                                                    "include_deleted_channels": True, "page": 0, "per_page": 20}))
    step("files_channel_files_name")(lambda: search(u2, {"terms": f"in:sqldiff-files-{impl}", "page": 0, "per_page": 20}))
    step("files_search_all_teams")(lambda: shape(call(u1, "POST", "/files/search", {"terms": "blue", "page": 0, "per_page": 20}),
                                                 lambda v: sorted(v)))
    step("files_search_not_member")(lambda: search(u3, {"terms": f"channel:{ctx['fch']}", "page": 0, "per_page": 20}))

    # ---- deleting a post with files ----
    step("files_post_delete")(lambda: call(u1, "DELETE", f"/posts/{ctx['fp2']}"))
    step("files_info_after_delete")(lambda: call(u1, "GET", f"/files/{ctx['two0']}/info"))
    step("files_get_after_delete")(lambda: raw(u1, f"/files/{ctx['two0']}"))
    step("files_search_after_delete")(lambda: search(u1, {"terms": "blue", "page": 0, "per_page": 20}))

    # ---- custom emoji ----
    def emoji_create(token, name, creator, image=red):
        body, ctype = multipart([("emoji", json.dumps({"name": name, "creator_id": creator}))],
                                [("image", "e.png", "image/png", image)])
        return call(token, "POST", "/emoji", raw=body, ctype=ctype)

    step("emoji_create")(lambda: shape(keep("emoji", emoji_create(u1, "sqldiffmoji", u1_id))))
    step("emoji_create_dup")(lambda: emoji_create(u1, "sqldiffmoji", u1_id))
    step("emoji_create_other_creator")(lambda: emoji_create(u1, "sqldiffother", u2_id))
    step("emoji_create_system_name")(lambda: emoji_create(u1, "smile", u1_id))
    step("emoji_create_big")(lambda: shape(emoji_create(u2, "sqldiffbig", u2_id, blue)))
    step("emoji_create_second")(lambda: shape(emoji_create(u2, "sqldiffsecond", u2_id, png(32, 32, (0, 200, 0)))))
    step("emoji_list")(lambda: shape(call(u1, "GET", "/emoji?page=0&per_page=50&sort=name")))
    step("emoji_by_name")(lambda: shape(call(u1, "GET", "/emoji/name/sqldiffmoji")))
    step("emoji_get")(lambda: shape(call(u1, "GET", f"/emoji/{ctx['emoji']}")))
    step("emoji_image")(lambda: raw(u2, f"/emoji/{ctx['emoji']}/image"))
    step("emoji_names")(lambda: shape(call(u1, "POST", "/emoji/names", ["sqldiffmoji", "nosuchone"])))
    step("emoji_autocomplete")(lambda: shape(call(u1, "GET", "/emoji/autocomplete?name=sqldiff")))
    step("emoji_search")(lambda: shape(call(u1, "POST", "/emoji/search", {"term": "sqldiff", "prefix_only": True})))
    step("emoji_react")(lambda: call(u2, "POST", "/reactions", {"user_id": u2_id, "post_id": ctx["fp"], "emoji_name": "sqldiffmoji"}))
    step("emoji_delete_other")(lambda: call(u2, "DELETE", f"/emoji/{ctx['emoji']}"))
    step("emoji_delete")(lambda: shape(call(u1, "DELETE", f"/emoji/{ctx['emoji']}")))
    step("emoji_delete_admin")(lambda: call(admin, "DELETE", f"/emoji/{call(u1, 'GET', '/emoji/name/sqldiffsecond')[1]['id']}"))
    step("emoji_get_deleted")(lambda: call(u1, "GET", "/emoji/name/sqldiffmoji"))
    step("emoji_image_deleted")(lambda: raw(u1, f"/emoji/{ctx['emoji']}/image"))

    # ---- profile image ----
    def set_image(token, uid, data, ctype="image/png", fname="me.png"):
        body, ct = multipart([], [("image", fname, ctype, data)])
        return call(token, "POST", f"/users/{uid}/image", raw=body, ctype=ct)

    step("user_image_default")(lambda: raw(u2, f"/users/{u1_id}/image"))
    step("user_image_set")(lambda: set_image(u1, u1_id, red))
    step("user_image_get")(lambda: raw(u2, f"/users/{u1_id}/image"))
    step("user_image_user")(lambda: shape(call(u2, "GET", f"/users/{u1_id}"), lambda v: {"last_picture_update": v.get("last_picture_update", "missing") != 0}))
    step("user_image_set_other")(lambda: set_image(u2, u1_id, red))
    step("user_image_set_text")(lambda: set_image(u1, u1_id, text, "text/plain", "x.txt"))
    step("user_image_get_default")(lambda: raw(u2, f"/users/{u1_id}/image/default"))
    step("user_image_reset")(lambda: call(u1, "DELETE", f"/users/{u1_id}/image"))
    step("user_image_after_reset")(lambda: raw(u2, f"/users/{u1_id}/image"))
    step("user_image_user_after")(lambda: shape(call(u2, "GET", f"/users/{u1_id}"), lambda v: mask({"last_picture_update": v.get("last_picture_update", "missing")})))

    # ---- team icon ----
    def set_icon(token, data):
        body, ct = multipart([], [("image", "icon.png", "image/png", data)])
        return call(token, "POST", f"/teams/{team}/image", raw=body, ctype=ct)

    step("team_icon_none")(lambda: raw(u1, f"/teams/{team}/image"))
    step("team_icon_set_member")(lambda: set_icon(u1, red))
    step("team_icon_set")(lambda: set_icon(admin, blue))
    step("team_icon_get")(lambda: raw(u1, f"/teams/{team}/image"))
    step("team_icon_team")(lambda: shape(call(u1, "GET", f"/teams/{team}"), lambda v: {"last_team_icon_update": bool(v.get("last_team_icon_update"))}))
    step("team_icon_remove")(lambda: call(admin, "DELETE", f"/teams/{team}/image"))
    step("team_icon_after_remove")(lambda: raw(u1, f"/teams/{team}/image"))
    step("team_icon_team_after")(lambda: shape(call(u1, "GET", f"/teams/{team}"), lambda v: mask({"last_team_icon_update": v.get("last_team_icon_update", "missing")})))
