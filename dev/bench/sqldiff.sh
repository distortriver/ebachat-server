#!/bin/bash
# tools/diff/sqldiff.py against Go and then Rust, each from the same snapshot of the data, then compared.
# sqldiff.sh [impls]; output in run/results/sqldiff-<impl>.json and sqldiff.txt.
set -euo pipefail
. "$(dirname "$0")/env.sh"
docker exec bench-db psql -qAt -U mmuser -d mattermost -c "SELECT 1 FROM pg_extension WHERE extname = 'pg_stat_statements'" | grep -q 1 \
  || { PGSS=1 "$BENCH/db.sh" restart; }
"$BENCH/server.sh" stop
pg() { docker exec bench-db psql -qAt -U mmuser -d postgres -c "$1"; }
pg "DROP DATABASE IF EXISTS sqldiff_snap"
pg "CREATE DATABASE sqldiff_snap TEMPLATE mattermost"
for impl in ${1:-go rust}; do
  "$BENCH/server.sh" stop
  pg "DROP DATABASE mattermost WITH (FORCE)"
  pg "CREATE DATABASE mattermost TEMPLATE sqldiff_snap"
  "$BENCH/server.sh" "$impl" 4
  sleep 3
  python3 "$REPO/tools/diff/sqldiff.py" run "$impl" "$RUN/results/sqldiff-$impl.json"
done
"$BENCH/server.sh" stop
# Back to the data as they were.
pg "DROP DATABASE mattermost WITH (FORCE)"
pg "CREATE DATABASE mattermost TEMPLATE sqldiff_snap"
python3 "$REPO/tools/diff/sqldiff.py" compare "$RUN/results/sqldiff-go.json" "$RUN/results/sqldiff-rust.json" | tee "$RUN/results/sqldiff.txt"
