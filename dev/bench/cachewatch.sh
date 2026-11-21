#!/bin/bash
# The Rust caches' misses every 5 s while one step runs: cachewatch.sh <cores> <rps:pps> <seconds> [cache]
set -euo pipefail
. "$(dirname "$0")/env.sh"
cores=$1 step=$2 secs=$3 name=${4:-members_for_user}
"$BENCH/server.sh" rust "$cores" >/dev/null
T=$(curl -si -X POST localhost:8065/api/v4/users/login -d '{"login_id":"admin","password":"Admin-pass-123"}' | tr -d '\r' | awk -F': ' 'tolower($1)=="token"{print $2}')
(cd "$REPO" && taskset -c "$LG_CPUS" ./target/release/loadgen steps http://localhost:8065 "$(cat "$RUN/server.pid")" "$step" "$secs") > "$RUN/cachewatch.out" 2>&1 &
lg=$!
while kill -0 $lg 2>/dev/null; do
  curl -s -H "Authorization: Bearer $T" localhost:8065/api/v4/ebachat/caches \
    | python3 -c "import json,sys,time; d=json.load(sys.stdin)['$name']; print(time.strftime('%T'), d)"
  sleep 5
done
grep -E '^ +[0-9]' "$RUN/cachewatch.out"
"$BENCH/server.sh" stop
