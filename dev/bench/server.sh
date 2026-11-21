#!/bin/bash
# server.sh go|rust <cores> — stops whichever server runs, starts this one on cores 0..cores-1, waits for ping.
# server.sh stop — stops it.
set -euo pipefail
. "$(dirname "$0")/env.sh"
if [ -f "$RUN/server.pid" ]; then
  kill "$(cat "$RUN/server.pid")" 2>/dev/null || true
  for _ in $(seq 50); do kill -0 "$(cat "$RUN/server.pid")" 2>/dev/null || break; sleep 0.2; done
  rm -f "$RUN/server.pid"
fi
[ "${1:-}" = stop ] && exit 0
impl=$1 cores=${2:-$SRV_MAX}
cpus=0-$((cores - 1))
ulimit -n 1048576
case $impl in
  go)
    cd "$BENCH/go"  # i18n, templates and client are found relative to it
    taskset -c "$cpus" ./bin/mattermost server --config "$RUN/config.json" > "$RUN/go.log" 2>&1 &
    ;;
  rust)
    cd "$REPO"
    MM_CONFIG=$RUN/config.json DATABASE_URL="postgres://mmuser:mmpass@localhost:5433/mattermost?sslmode=disable" \
      DB_MAX_CONNECTIONS=100 LISTEN=0.0.0.0:8065 \
      taskset -c "$cpus" ./target/release/ebachat-server > "$RUN/rust.log" 2>&1 &
    ;;
  *) echo "go|rust|stop"; exit 1 ;;
esac
echo $! > "$RUN/server.pid"
for _ in $(seq 300); do
  curl -sf localhost:8065/api/v4/system/ping >/dev/null && { echo "$impl on cores $cpus, pid $(cat "$RUN/server.pid")"; exit 0; }
  kill -0 "$(cat "$RUN/server.pid")" 2>/dev/null || { echo "$impl died"; tail -20 "$RUN/$impl.log"; exit 1; }
  sleep 0.5
done
echo "$impl did not answer"; exit 1
