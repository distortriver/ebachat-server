#!/bin/bash
# server.sh go|rust <cores> — stops whichever server runs, starts this one on cores 0..cores-1, waits for ping.
# server.sh stop — stops it.
# PORT (8065), DB (mattermost), CPUS (0..cores-1) and NAME (server: its pid file, log and config copy) let two
# servers run side by side on two databases.
set -euo pipefail
. "$(dirname "$0")/env.sh"
PORT=${PORT:-8065} DB=${DB:-mattermost} NAME=${NAME:-server}
# SERVER_ENV="K=V K=V": settings over env.sh's (a scenario that needs them, see sqldiff.py env).
for kv in ${SERVER_ENV:-}; do export "$kv"; done
pidf=$RUN/$NAME.pid
if [ -f "$pidf" ]; then
  kill "$(cat "$pidf")" 2>/dev/null || true
  for _ in $(seq 50); do kill -0 "$(cat "$pidf")" 2>/dev/null || break; sleep 0.2; done
  rm -f "$pidf"
fi
[ "${1:-}" = stop ] && exit 0
impl=$1 cores=${2:-$SRV_MAX}
cpus=${CPUS:-0-$((cores - 1))}
cfg=$RUN/config.json
if [ "$NAME" != server ]; then
  cfg=$RUN/config-$NAME.json
  cp "$RUN/config.json" "$cfg"
  mkdir -p "$RUN/data-$NAME"
  export MM_FILESETTINGS_DIRECTORY=$RUN/data-$NAME/
fi
export MM_SQLSETTINGS_DATASOURCE="postgres://mmuser:mmpass@localhost:5433/$DB?sslmode=disable&connect_timeout=10"
export MM_SERVICESETTINGS_LISTENADDRESS=:$PORT
case $impl in
  go)
    cd "$BENCH/go"  # i18n, templates and client are found relative to it
    taskset -c "$cpus" ./bin/mattermost server --config "$cfg" > "$RUN/$NAME-go.log" 2>&1 &
    ;;
  rust)
    cd "$REPO"
    MM_CONFIG=$cfg DATABASE_URL="postgres://mmuser:mmpass@localhost:5433/$DB?sslmode=disable" \
      DB_MAX_CONNECTIONS=100 LISTEN=0.0.0.0:$PORT \
      taskset -c "$cpus" ./target/release/ebachat-server > "$RUN/$NAME-rust.log" 2>&1 &
    ;;
  *) echo "go|rust|stop"; exit 1 ;;
esac
echo $! > "$pidf"
for _ in $(seq 300); do
  curl -sf localhost:$PORT/api/v4/system/ping >/dev/null && { echo "$impl on cores $cpus, port $PORT, db $DB, pid $(cat "$pidf")"; exit 0; }
  kill -0 "$(cat "$pidf")" 2>/dev/null || { echo "$impl died"; tail -20 "$RUN/$NAME-$impl.log"; exit 1; }
  sleep 0.5
done
echo "$impl did not answer"; exit 1
