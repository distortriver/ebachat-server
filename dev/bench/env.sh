# Sourced by the bench scripts. CPU layout: the server on the first half of the cores, the load generator on the
# last 3/16 (at least 2), Postgres on what is left (8 vCPU: 0-3, 4-5, 6-7; 16 vCPU: 0-7, 8-12, 13-15).
BENCH=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$BENCH/../.." && pwd)
RUN=$BENCH/run
mkdir -p "$RUN/results" "$RUN/data"
# Lost on reboot: many sockets on loopback.
sudo sysctl -q -w net.core.somaxconn=8192 net.ipv4.ip_local_port_range="10000 65000" net.ipv4.tcp_tw_reuse=1
# 500 websockets plus the HTTP connections of a thousand requests a second: far beyond the default 1024.
ulimit -n "$(ulimit -Hn)"
N=$(nproc)
SRV_MAX=$((N / 2))
LG_N=$((N * 3 / 16)); [ "$LG_N" -lt 2 ] && LG_N=2
PG_CPUS=$SRV_MAX-$((N - LG_N - 1))
LG_CPUS=$((N - LG_N))-$((N - 1))
export LG_DB=postgres://mmuser:mmpass@localhost:5433/mattermost
export LG_TOKENS=$RUN/tokens.tsv
export LG_PG_CONTAINER=bench-db
# Both servers: the same settings over the same config.json.
export MM_SQLSETTINGS_DRIVERNAME=postgres
export MM_SQLSETTINGS_DATASOURCE="postgres://mmuser:mmpass@localhost:5433/mattermost?sslmode=disable&connect_timeout=10"
export MM_SQLSETTINGS_MAXOPENCONNS=100
export MM_SERVICESETTINGS_SITEURL=http://localhost:8065
export MM_SERVICESETTINGS_LISTENADDRESS=:8065
export MM_TEAMSETTINGS_ENABLEOPENSERVER=true
export MM_TEAMSETTINGS_MAXUSERSPERTEAM=10000
export MM_PLUGINSETTINGS_ENABLE=false
export MM_FILESETTINGS_DIRECTORY=$RUN/data/
export MM_LOGSETTINGS_ENABLEFILE=false
export MM_LOGSETTINGS_CONSOLELEVEL=WARN
export MM_METRICSSETTINGS_ENABLE=false
export MM_EMAILSETTINGS_SENDEMAILNOTIFICATIONS=false
export MM_EMAILSETTINGS_SENDPUSHNOTIFICATIONS=false
export MM_RATELIMITSETTINGS_ENABLE=false
