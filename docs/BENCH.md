# Go vs Rust: first measurement

2026-11-19. The first slice of the Rust server against Mattermost Team Edition 12.1.0 (Go), on one database, under
the same load.

## Setup

- **Machine:** Apple M1, 16 GB, macOS.
- **Database:** Postgres 16 in docker (`make stand`, port 5433).
- **Both servers native arm64:**
  - Go: built from the 12.1.0 source with the release ldflags (`dev/bin/mattermost`, run by `dev/run-go.sh`, plugins
    off).
  - Rust: `cargo build --release`.
- **One server at a time:** each starts fresh, then the two scenarios run back to back.
- **Data:** 249 users, 40 public channels (about a third of the users in each), 30k posts. Seeded through the Go API
  (`loadgen seed`). Production has 1,200 users and 10M posts.
- **Load:** `loadgen run`, 100 req/s (about 5× production's 22 req/s), 15 s warm-up, then 120 s measured.
  - Every user keeps a websocket open, 249 in all.
  - Requests are spread over users and channels by a seeded RNG, so both servers get the same sequence.
- **Mix:** production's proportions (docs/prod-traffic.tsv) over the endpoints both servers have:
  - `status/ids` 35%
  - `members/me/view` 27%
  - `channels/{id}/posts?since` 20%
  - `users/ids` 14%
  - `users/me` 3%
  - `full` also creates posts: `POST /posts` 14% of requests, with the websocket `posted` fan-out.
- **Measured:**
  - RSS of the server process (`ps`, sampled every second);
  - CPU time of the server process and of the Postgres container (cgroup `cpu.stat`), over the measured window;
  - latency at the client.

## Results

| | Go 12.1.0 | Rust | Rust / Go |
| --- | ---: | ---: | ---: |
| RSS after start, idle | 163 MB | 7 MB | 1/23 |
| RSS under load, `read`, avg / max | 256 / 278 MB | 15 / 43 MB | 1/17 / 1/6 |
| RSS under load, `full`, avg / max | 309 / 319 MB | 39 / 44 MB | 1/8 / 1/7 |
| Server CPU, `read` | 13.9% of a core | 6.2% | 0.45 |
| Server CPU, `full` | 27.7% | 12.3% | 0.44 |
| Postgres CPU, `read` | 12.9% | 11.4% | 0.88 |
| Postgres CPU, `full` | 19.0% | 13.3% | 0.70 |
| Websocket events delivered, `full` | 184,151 | 184,151 | same |

Latency (ms), p50 / p99:

| Endpoint | Go `read` | Rust `read` | Go `full` | Rust `full` |
| --- | ---: | ---: | ---: | ---: |
| `status/ids` | 1.10 / 2.64 | 0.72 / 3.99 | 0.98 / 3.09 | 0.78 / 2.66 |
| `members/me/view` | 9.49 / 18.60 | 4.15 / 23.48 | 7.96 / 16.85 | 4.18 / 9.03 |
| `posts?since` | 0.85 / 5.69 | 4.30 / 15.66 | 6.47 / 14.74 | 4.10 / 8.99 |
| `users/ids` | 1.24 / 3.18 | 4.14 / 23.45 | 1.07 / 3.64 | 3.42 / 7.75 |
| `users/me` | 2.68 / 6.10 | 2.19 / 7.35 | 2.04 / 12.65 | 1.70 / 3.82 |
| `POST /posts` | | | 14.11 / 28.60 | 11.02 / 20.07 |

Raw output: `dev/run/bench/*.txt` (not tracked).

## What this does and does not show

- **Memory is the clear result:** about 7–20× less RSS. Go's heap grows with its caches and GC headroom. In
  production, Go holds 825 MB anon at 22 req/s.
- **The CPU result is an upper bound for Rust: it does less work than Go in this slice.**
  - `POST /posts` in Rust: insert, channel counters, `posted` fan-out, mark viewed. Go also runs notifications
    (mentions, channel profiles and notify props), audit, link previews and webhooks.
  - `posts?since` in Rust returns empty `metadata`. Go prepares it (emojis, files, reactions, priority,
    acknowledgements).
  - In `read`, the work is close on `status/ids`, `view`, `users/ids` and `me`. Even there Rust uses less than half
    the CPU.
- **Go is faster where it caches and Rust has no cache yet:**
  - `users/ids`: user cache;
  - `posts?since`: the last-post-time cache answers "nothing new" without a query.

  Both are easy to add. They are also why Postgres does a little more work per such request under Rust.
- **Postgres CPU is lower under Rust**, mostly because of what `POST /posts` does not do yet.
- **Small data:** 30k posts. The thread scans that dominate production's Postgres CPU are not exercised here.
- **Noise:** one run each, on a laptop. Expect noise of ±10–15%.

## Reproduce

```sh
make stand && docker compose -f dev/docker-compose.yml stop mm   # Postgres only; native Go instead of the container
./dev/run-go.sh &                                                # Go on :8065
cargo build --release
./target/release/loadgen seed 249 40 30000                       # 12.1 without a license stops at 250 users
./target/release/loadgen run http://localhost:8065 <go pid> read 100 120
./target/release/loadgen run http://localhost:8065 <go pid> full 100 120
# stop Go, then: LISTEN=127.0.0.1:8066 ./target/release/ebachat-server &
./target/release/loadgen run http://localhost:8066 <rust pid> read 100 120
./target/release/loadgen run http://localhost:8066 <rust pid> full 100 120
```

To build Go natively, use `go build` in `server/` with a `go.work` that uses `server` and `server/public` (Team
Edition has no arm64 image). See the ldflags in `dev/README.md`.
