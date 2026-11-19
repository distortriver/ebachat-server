#!/bin/sh
# The Go server built natively (dev/bin/mattermost, see dev/README.md) on the stand's database, port 8065.
# Run it instead of the `mm` container: never two servers on one database at once.
set -e
here=$(cd "$(dirname "$0")" && pwd)
MM_SRC=${MM_SRC:-$here/../../ebac-messenger/mattermost/server}
run=$here/run/go
mkdir -p "$run/data"
export MM_SQLSETTINGS_DRIVERNAME=postgres
export MM_SQLSETTINGS_DATASOURCE="postgres://mmuser:mmpass@localhost:5433/mattermost?sslmode=disable&connect_timeout=10"
export MM_SERVICESETTINGS_SITEURL=http://localhost:8065
export MM_SERVICESETTINGS_LISTENADDRESS=:8065
export MM_SERVICESETTINGS_ENABLELOCALMODE=true
export MM_SERVICESETTINGS_LOCALMODESOCKETLOCATION=$run/mattermost_local.socket
export MM_TEAMSETTINGS_ENABLEOPENSERVER=true
export MM_TEAMSETTINGS_MAXUSERSPERTEAM=10000
export MM_PLUGINSETTINGS_ENABLE=false
export MM_FILESETTINGS_DIRECTORY=$run/data/
export MM_LOGSETTINGS_ENABLEFILE=false
export MM_LOGSETTINGS_CONSOLELEVEL=WARN
export MM_METRICSSETTINGS_ENABLE=false
# i18n and templates are found relative to the working directory.
cd "$MM_SRC"
exec "$here/bin/mattermost" server --config "$run/config.json"
