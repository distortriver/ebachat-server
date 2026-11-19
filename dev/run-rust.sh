#!/bin/sh
# The Rust server on :8066 against the stand's database and the Go server's config; pid in dev/run/rust.pid.
cd "$(dirname "$0")/.."
[ -f dev/run/rust.pid ] && kill "$(cat dev/run/rust.pid)" 2>/dev/null && sleep 0.3
MM_CONFIG=${MM_CONFIG:-dev/run/go/config.json} \
DATABASE_URL=${DATABASE_URL:-postgres://mmuser:mmpass@localhost:5433/mattermost?sslmode=disable} \
LISTEN=${LISTEN:-127.0.0.1:8066} \
nohup ./target/${PROFILE:-release}/ebachat-server > dev/run/rust.log 2>&1 &
echo $! > dev/run/rust.pid
