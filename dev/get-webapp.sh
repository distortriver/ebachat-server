#!/bin/sh
# The released web client of 12.1.0 (Apache-2.0) into dev/webapp, linked as dev/client: the Go server finds
# client/ next to its binary's directory, the Rust server through EBACHAT_CLIENT_DIR (dev/stand.env).
set -e
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$here/webapp" && cd "$here/webapp"
curl -sSfL https://releases.mattermost.com/12.1.0/mattermost-team-12.1.0-linux-amd64.tar.gz | tar -xzf - mattermost/client
ln -sfn webapp/mattermost/client "$here/client"
