# Deploying the Gateway on a Linux server

Every push to `main` runs `.github/workflows/gateway-release.yml`. It tests the
server crates, builds `claude-web-ui-server` for Linux x86_64, and uploads the
binary, its SHA-256 checksum, and `version.txt` to the rolling
`gateway-latest` release. A systemd timer on the server runs `update.sh` every
five minutes. When the release names a new commit, the script downloads the
binary, verifies the checksum, swaps it in, and restarts the Gateway. If the new
build does not answer its health check within 30 seconds, the script restores
the previous binary and does not retry that commit. A restart interrupts CLI
turns that are running at that moment.

## Layout

| Path | Purpose |
| --- | --- |
| `/opt/claude-web-ui/bin/claude-web-ui-server` | Gateway binary (`.prev` is the previous build) |
| `/opt/claude-web-ui/update.sh` | Updater, a copy of `deploy/update.sh` |
| `/opt/claude-web-ui/version` | Commit of the installed build |
| `/etc/claude-web-ui.env` | Environment for the service |
| `/var/lib/claude-web-ui` | Data directory: SQLite database, JWT secret, CLI profiles |

The service runs as the `claude-web` user. Keep project workspaces readable and
writable by it.

## Install

As root on an Ubuntu or Debian server:

```sh
useradd --system --create-home --home-dir /var/lib/claude-web-ui --shell /bin/bash claude-web
install -d -o root -g root /opt/claude-web-ui /opt/claude-web-ui/bin
install -m 0755 deploy/update.sh /opt/claude-web-ui/update.sh
install -m 0644 deploy/claude-web-ui.service deploy/claude-web-ui-update.service \
  deploy/claude-web-ui-update.timer /etc/systemd/system/
cat > /etc/claude-web-ui.env <<'EOF'
PATH=/var/lib/claude-web-ui/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
CLAUDE_WEB_UI_BIND=127.0.0.1:8090
CLAUDE_WEB_UI_DATA_DIR=/var/lib/claude-web-ui/data
CLAUDE_WEB_UI_HEALTH_URL=http://127.0.0.1:8090/api/auth/status
# CLAUDE_WEB_UI_TELEGRAM_TOKEN=
# CLAUDE_WEB_UI_DISCORD_TOKEN=
EOF
chmod 0640 /etc/claude-web-ui.env
chgrp claude-web /etc/claude-web-ui.env
systemctl daemon-reload
systemctl enable claude-web-ui
systemctl enable --now claude-web-ui-update.timer
systemctl start claude-web-ui-update.service
```

The updater unit reads the same environment file, so `CLAUDE_WEB_UI_HEALTH_URL`
must match `CLAUDE_WEB_UI_BIND`.

## CLIs

The Gateway runs `claude`, `codex`, and `agy` from `PATH`. Install them for the
service user with the official installers, which place them in
`~/.local/bin` (already first on the service `PATH` above) and update
themselves:

```sh
cd /tmp
curl -fsSL -o claude.sh https://claude.ai/install.sh
curl -fsSL -o codex.sh https://chatgpt.com/codex/install.sh
curl -fsSL -o agy.sh https://antigravity.google/cli/install.sh
for cli in claude codex agy; do
  runuser -u claude-web -- env HOME=/var/lib/claude-web-ui bash -c "cd ~ && bash /tmp/$cli.sh"
done
```

Sign the CLI accounts in from **Admin: CLI Accounts** in the dashboard.

## First administrator

The first person to open an uninitialized Gateway creates the administrator.
Create it before the site is public, over an SSH tunnel:

```sh
ssh -L 8090:127.0.0.1:8090 root@<server>
```

Then open http://localhost:8090.

## HTTPS

Point a DNS record at the server and add a Caddy site for it:

```caddyfile
cwu.example.com {
	encode zstd gzip

	# The first administrator is created over the SSH tunnel only.
	@bootstrap path /api/auth/bootstrap
	respond @bootstrap 403

	reverse_proxy 127.0.0.1:8090 {
		flush_interval -1
		transport http {
			read_timeout 30m
			write_timeout 30m
		}
	}
}
```

Caddy proxies the chat, transcript, and terminal WebSockets without extra
configuration.

## Operations

- Logs: `journalctl -u claude-web-ui -f`, updates: `journalctl -u claude-web-ui-update`
- Update now: `systemctl start claude-web-ui-update.service`
- Roll back by hand: `mv /opt/claude-web-ui/bin/claude-web-ui-server.prev /opt/claude-web-ui/bin/claude-web-ui-server && systemctl restart claude-web-ui`
