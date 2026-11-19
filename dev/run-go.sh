#!/bin/sh
# The Go server built natively (dev/bin/mattermost, see dev/README.md) on the stand's database, port 8065.
# Run it instead of the `mm` container: never two servers on one database at once.
set -e
here=$(cd "$(dirname "$0")" && pwd)
MM_SRC=${MM_SRC:-$here/../../ebac-messenger/mattermost/server}
run=$here/run/go
. "$here/stand.env"
export MM_SERVICESETTINGS_LISTENADDRESS=:8065
# i18n and templates are found relative to the working directory.
cd "$MM_SRC"
exec "$here/bin/mattermost" server --config "$run/config.json"
