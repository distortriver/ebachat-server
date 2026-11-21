#!/bin/bash
# Postgres 16 on its own cores, on loopback only. `db.sh reset` starts from an empty database.
set -euo pipefail
. "$(dirname "$0")/env.sh"
if [ "${1:-}" = reset ]; then
  docker rm -f bench-db >/dev/null 2>&1 || true
  docker volume rm bench-db >/dev/null 2>&1 || true
fi
if ! docker ps --format '{{.Names}}' | grep -qx bench-db; then
  docker rm -f bench-db >/dev/null 2>&1 || true
  docker run -d --name bench-db --cpuset-cpus="$PG_CPUS" --shm-size=1g \
    -e POSTGRES_USER=mmuser -e POSTGRES_PASSWORD=mmpass -e POSTGRES_DB=mattermost \
    -p 127.0.0.1:5433:5432 -v bench-db:/var/lib/postgresql/data postgres:16-alpine \
    -c max_connections=400 -c shared_buffers=3GB -c effective_cache_size=8GB -c work_mem=16MB \
    -c autovacuum=off >/dev/null
fi
until docker exec bench-db pg_isready -q -U mmuser -d mattermost; do sleep 1; done
# No autovacuum, so that it cannot land in one run and not another; statistics refreshed by hand instead.
if [ "${1:-}" = analyze ]; then
  docker exec bench-db psql -q -U mmuser -d mattermost -c 'VACUUM ANALYZE'
fi
echo "postgres up on cores $PG_CPUS, autovacuum $(docker exec bench-db psql -U mmuser -d mattermost -Atc 'SHOW autovacuum')"
