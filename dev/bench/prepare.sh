#!/bin/bash
# An empty database, Go's first start (schema and config.json), then the data seeded through Rust.
set -euo pipefail
. "$(dirname "$0")/env.sh"
"$BENCH/db.sh" reset
rm -f "$RUN/config.json"
"$BENCH/server.sh" go "$SRV_MAX"
"$BENCH/server.sh" rust "$SRV_MAX"
cd "$REPO"
LG_SEED_URL=http://localhost:8065 taskset -c "$LG_CPUS" ./target/release/loadgen seed "${USERS:-500}" 40 "${POSTS:-30000}"
docker exec bench-db psql -U mmuser -d mattermost -Atc \
  "SELECT 'users ' || count(*) FROM Users UNION ALL SELECT 'posts ' || count(*) FROM Posts UNION ALL
   SELECT 'bench-public members ' || count(*) FROM ChannelMembers cm JOIN Channels c ON c.Id = cm.ChannelId WHERE c.Name = 'bench-public'"
"$BENCH/server.sh" stop
