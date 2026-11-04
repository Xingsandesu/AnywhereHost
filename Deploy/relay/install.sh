#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
cargo build --release --locked -p anywhere-relay
install -m 0755 target/release/anywhere-relay /usr/local/bin/anywhere-relay
id iroh-relay >/dev/null 2>&1 || useradd --system --home-dir /var/lib/iroh-relay --shell /usr/sbin/nologin iroh-relay
install -d -m 0755 /etc/iroh-relay
install -m 0644 Deploy/relay/relay.toml /etc/iroh-relay/relay.toml
install -m 0644 Deploy/relay/iroh-relay.service /etc/systemd/system/iroh-relay.service
systemctl daemon-reload
printf '配置 TLS 证书及 443 反向代理后执行 systemctl enable --now iroh-relay\n'
