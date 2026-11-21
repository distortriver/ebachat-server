#!/bin/bash
# perop.py against each server on the same data, pg_stat_statements loaded; output in run/results/perop-<impl>.json.
set -euo pipefail
. "$(dirname "$0")/env.sh"
docker exec bench-db psql -qAt -U mmuser -d mattermost -c "SELECT 1 FROM pg_extension WHERE extname = 'pg_stat_statements'" | grep -q 1 \
  || { PGSS=1 "$BENCH/db.sh" restart; }
for impl in ${1:-rust go}; do
  "$BENCH/server.sh" "$impl" 4
  sleep 5
  taskset -c "$LG_CPUS" python3 "$BENCH/perop.py" "$impl" "$RUN/results/perop-$impl.json" "${N:-40}"
done
"$BENCH/server.sh" stop
