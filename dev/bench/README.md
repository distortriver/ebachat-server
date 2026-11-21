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
