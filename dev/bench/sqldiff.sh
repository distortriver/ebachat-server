#!/bin/bash
# tools/diff/sqldiff.py against Go and Rust at once, each on its own copy of one snapshot of the data, then compared.
# sqldiff.sh [TAG] [PORT]: TAG (default main) names the databases, logs and results, so several runs can go at once;
# Go listens on PORT (default 8065), Rust on PORT+1. SQLDIFF_ONLY / SQLDIFF_UPTO pass through to sqldiff.py.
# Output: run/results/sqldiff-TAG-{go,rust}.json and sqldiff-TAG.txt.
set -euo pipefail
. "$(dirname "$0")/env.sh"
tag=${1:-main} port=${2:-8065}
docker exec bench-db psql -qAt -U mmuser -d mattermost -c "SELECT 1 FROM pg_extension WHERE extname = 'pg_stat_statements'" | grep -q 1 \
  || { PGSS=1 "$BENCH/db.sh" restart; }
pg() { docker exec bench-db psql -qAt -U mmuser -d postgres -c "$1"; }
# The snapshot is the bench data as they stand (mattermost), taken once per tag. CREATE DATABASE ... TEMPLATE fails
# while anyone else reads the template, so parallel runs take their copies one at a time.
exec 9>"$RUN/sqldiff.lock"
flock 9
pg "DROP DATABASE IF EXISTS sqldiff_${tag}_snap"
pg "CREATE DATABASE sqldiff_${tag}_snap TEMPLATE mattermost" 2>/dev/null || {
  # mattermost busy (a server on it): copy through a dump instead
  pg "CREATE DATABASE sqldiff_${tag}_snap"
  docker exec bench-db sh -c "pg_dump -U mmuser mattermost | psql -q -U mmuser sqldiff_${tag}_snap" >/dev/null
}
i=0
for impl in go rust; do
  db=sqldiff_${tag}_$impl
  pg "DROP DATABASE IF EXISTS $db WITH (FORCE)"
  pg "CREATE DATABASE $db TEMPLATE sqldiff_${tag}_snap"
  [ "$impl" = rust ] && flock -u 9
  # each server on its own cores out of the server half
  PORT=$((port + i)) DB=$db NAME=sqldiff-$tag-$impl CPUS=$((i * 2))-$((i * 2 + 1)) "$BENCH/server.sh" "$impl" 2 &
  i=$((i + 1))
done
wait
sleep 2
i=0
for impl in go rust; do
  SQLDIFF_URL=http://localhost:$((port + i)) SQLDIFF_DB=sqldiff_${tag}_$impl \
    python3 "$REPO/tools/diff/sqldiff.py" run "$impl" "$RUN/results/sqldiff-$tag-$impl.json" > "$RUN/sqldiff-$tag-$impl.log" 2>&1 &
  i=$((i + 1))
done
wait
for impl in go rust; do NAME=sqldiff-$tag-$impl "$BENCH/server.sh" stop; done
for impl in go rust; do pg "DROP DATABASE IF EXISTS sqldiff_${tag}_$impl WITH (FORCE)"; done
pg "DROP DATABASE IF EXISTS sqldiff_${tag}_snap"
python3 "$REPO/tools/diff/sqldiff.py" compare "$RUN/results/sqldiff-$tag-go.json" "$RUN/results/sqldiff-$tag-rust.json" \
  | tee "$RUN/results/sqldiff-$tag.txt"
