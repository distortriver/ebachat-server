# Load bench on a dedicated VM

Go 12.1.0 and the Rust server, one at a time, on one Postgres, with the load generator on the same VM but on its
own cores. Everything runs in the `bench` tmux session on the VM, so a dropped ssh loses nothing:
`ssh <vm> -t tmux attach -t bench`.

- `setup.sh`: packages, Rust, the Go 12.1.0 binary from the official image, the release build. Idempotent.
- `env.sh`: the CPU layout (server / Postgres / load generator) and the settings both servers share.
- `db.sh`: Postgres 16 pinned to its cores.
- `server.sh go|rust <cores>`: one server pinned to its first `<cores>` cores; pid in `run/server.pid`.
- `prepare.sh`: first start of Go (schema, config.json), then 500 users, 40 channels and `bench-public` (every
  user in it) seeded through the Rust server, which has no 250-user limit. Go only checks the limit when a user
  is created or activated, so it serves them as well.
- `scale.sh`, `fanout.sh`: the two scenarios; output in `run/results/`.

## Go vs Rust on client scenarios (sqldiff)

`sqldiff.sh TAG PORT` runs `tools/diff/sqldiff.py` against Go (PORT) and Rust (PORT+1) side by side, each on its own
copy of one snapshot of the bench data, and prints per step what differs: the answer (status and keys, or the whole
body with ids and times masked for steps that ask for it), the rows written per table and operation
(pg_stat_statements), and the websocket events three observers got. Scenarios are `tools/diff/scen_<name>.py`
(`SQLDIFF_SCENARIO=name`): `core`, `files`, `posts`, `admin`, `live`, `chan`, `reads`. Several runs can go at once
with different tags and ports. `SQLDIFF_ONLY=prefix,...` records only matching steps, `SQLDIFF_UPTO=step` stops.
`notify` (search and notifications) runs an SMTP sink and needs email on: a module's `SERVER_ENV` reaches the servers'
environment (`sqldiff.py env`, `server.sh` `SERVER_ENV`), since env.sh's `MM_*` overrides beat `PUT /config`.

All seven at once (about ten minutes):
```sh
p=8100; for t in core files posts admin live chan reads; do SQLDIFF_SCENARIO=$t ./dev/bench/sqldiff.sh $t $p > run/$t.txt 2>&1 & p=$((p+2)); done; wait
```

Differences that remain on purpose (2026-11-21, each confirmed by the project owner):
- Go 12.1 without a license refuses to create or activate users beyond 250 (`user_limit.exceeded`, `limits/server`);
  Rust has no such cap (`core` user_activate, `admin` bot enable, `reads` limits).
- After a team join, Go sends the default channels' `posted`/`user_added` 0 or 1 times depending on its connection's
  stale membership snapshot; Rust sends them all (docs/CACHES.md).
- Converting a bot back to a user: Go's answer keeps `is_bot` from a stale cache (docs/USERS.md).
- Invalid channel notify props: Go stores them and then fails every later member update; Rust refuses them
  (docs/CHANNELS.md). A PUT with no display name: Go posts a "display name changed" message for a name it keeps.
- Files of a post: the released Go binary returns them in a changing order; Rust keeps the post's `file_ids` order,
  as the 12.1.0 source's `orderFileInfosByID` does.
- `GiphySdkKey` in `config/client`: a release Go build answers Mattermost's own embedded GIPHY key; Rust answers the
  configured `ServiceSettings.GiphySdkKey` (empty unless an admin sets one), never Mattermost's key.
- Guests (`guests`, the demotion done in the database since neither server demotes without a license): Go counts a
  guest's `users/stats` once per shared channel, and joins a promoted user to the default channels as a guest;
  Rust does neither (docs/USERS.md).
- Branding: every text a user sees names the site (`TeamSettings.SiteName`, else EBAChat) where Go says
  Mattermost, and emails have no Mattermost footer (docs/BRANDING.md): command descriptions, emails, some messages.
- Password reset: the token and email go to a background task and the answer is always 200; Go answers 500 when
  the mail cannot be sent, which reveals that the address has an account (docs/SECURITY.md).
- Post search pages for real; Go's database search answers the newest 100 on page 0 and nothing after
  (docs/SEARCH.md). Within the first 100 the posts and their order match, apart from posts sharing a `create_at`
  millisecond, whose order Go leaves undefined and Rust fixes by id.
- Noise: the built-in commands' order in `GET /commands` and autocomplete — Go's follows its map iteration and
  changes per process, Rust's is by trigger.
- Noise: `config/client` build fields of the bench's dev Go build; ordering by `last_activity_at`, which depends on
  each run's timing; image bytes (encoders differ, sizes and formats match). Adding several members at once: Go
  posts each "added to the channel" message from a goroutine, so a member added later in the same request may or
  may not get the `posted` of an earlier one (`notify` notify_search_setup).
