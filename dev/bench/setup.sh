#!/bin/bash
# Once per VM (idempotent): packages, Rust, Go 12.1.0 from the official image, the release build.
set -euo pipefail
. "$(dirname "$0")/env.sh"
sudo apt-get update -qq
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq docker.io build-essential pkg-config tmux sysstat jq rsync curl
sudo usermod -aG docker "$USER"
# Many sockets on loopback.
sudo sysctl -q -w net.core.somaxconn=8192 net.ipv4.ip_local_port_range="10000 65000" net.ipv4.tcp_tw_reuse=1 fs.file-max=1000000
grep -q 'nofile 1048576' /etc/security/limits.conf || echo "$USER - nofile 1048576" | sudo tee -a /etc/security/limits.conf
command -v cargo >/dev/null || [ -x ~/.cargo/bin/cargo ] || curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
. ~/.cargo/env
sudo docker pull -q postgres:16-alpine
if [ ! -x "$BENCH/go/bin/mattermost" ]; then
  sudo docker pull -q mattermost/mattermost-team-edition:12.1.0
  id=$(sudo docker create mattermost/mattermost-team-edition:12.1.0)
  sudo docker cp "$id:/mattermost" "$BENCH/go"
  sudo docker rm "$id" >/dev/null
  sudo chown -R "$USER" "$BENCH/go"
fi
cd "$REPO"
nice -n 10 cargo build --release -p ebachat-server -p loadgen
echo "setup done: $(nproc) cores; server 0-$((SRV_MAX - 1)), postgres $PG_CPUS, loadgen $LG_CPUS"
