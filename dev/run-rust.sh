#!/bin/sh
# The Rust server on :8066 with the stand's settings over the Go server's config; pid in dev/run/rust.pid.
here=$(cd "$(dirname "$0")" && pwd)
cd "$here/.."
run=$here/run/go
. "$here/stand.env"
[ -f dev/run/rust.pid ] && kill "$(cat dev/run/rust.pid)" 2>/dev/null && sleep 0.3
# SiteURL stays Go's (:8065) so both servers answer alike; browsers and the desktop app on :8066 send that origin.
MM_SERVICESETTINGS_ALLOWCORSFROM=${MM_SERVICESETTINGS_ALLOWCORSFROM:-http://localhost:8066} \
MM_CONFIG=${MM_CONFIG:-$run/config.json} \
DATABASE_URL=${DATABASE_URL:-postgres://mmuser:mmpass@localhost:5433/mattermost?sslmode=disable} \
LISTEN=${LISTEN:-127.0.0.1:8066} \
nohup ./target/${PROFILE:-release}/ebachat-server > dev/run/rust.log 2>&1 &
echo $! > dev/run/rust.pid
