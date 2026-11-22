# watchpost reference

**English** | [Türkçe](reference.tr.md) · [Back to the README](../README.md)

- [Commands](#commands)
- [Configuration reference](#configuration-reference)
- [Notification setup](#notification-setup)
- [Status page and API](#status-page-and-api)
- [Running as a systemd service](#running-as-a-systemd-service)
- [Running with Docker](#running-with-docker)

## Commands

| Command | What it does |
|---|---|
| `watchpost init [--force]` | Writes a commented example config to `watchpost.toml` (or to the path given with `--config`). |
| `watchpost check [--config-only]` | Validates the config, then runs every monitor once and prints a table. Exit code `0` if nothing is down, `1` if something is down, `2` on config errors. |
| `watchpost run` | Runs the scheduler, the notifications and the status page until stopped. |
| `watchpost test-notify [NAME]` | Sends a test message through every notifier, or only through `NAME`. |

Global options: `-c, --config <PATH>` (default `watchpost.toml`, or the
`WATCHPOST_CONFIG` environment variable), `--log-level error|warn|info|debug|trace`
(when not given, `RUST_LOG` is used, falling back to `info` for `run`) and
`--log-format text|json`.

## Configuration reference

A complete, commented example is in [`watchpost.example.toml`](../watchpost.example.toml).
`watchpost init` writes the same file.

Durations are strings such as `"30s"`, `"5m"`, `"1h"` or `"1h 30m"`. A plain
integer means seconds. Unknown keys are rejected, so typos are caught early.
`watchpost check` reports all problems at once.

### Environment variables in the config

Any string value may contain `${NAME}`, which is replaced with the value of
the environment variable `NAME`. If the variable is not set, the config fails
to load and the error names the variable. Other forms:

- `${NAME:-default}` uses `default` if `NAME` is unset or empty.
- `$${` gives a literal `${`.

Substitution happens on values only, never in comments.

```toml
bot_token = "${TELEGRAM_BOT_TOKEN}"
listen    = "${WATCHPOST_LISTEN:-127.0.0.1:8787}"
```

### Top level

| Key | Default | Description |
|---|---|---|
| `language` | `"en"` | `"en"` or `"tr"`. Used by the status page and notifications. |
| `title` | `"watchpost"` | Shown on the status page and in notification subjects. |

### `[web]`

| Key | Default | Description |
|---|---|---|
| `enabled` | `true` | Set to `false` to run without the web server. |
| `listen` | `"127.0.0.1:8787"` | Address and port. Use `"0.0.0.0:8787"` to listen on all interfaces. |
| `public_url` | – | If set, notifications include a link to the status page. |

### `[web.auth]` (optional)

| Key | Description |
|---|---|
| `username` | Basic-auth user name. |
| `password` | Basic-auth password. Use `${ENV}` here. |

When set, the page, the assets and `/api/status` require authentication.
`/healthz` always stays open. Basic auth sends the password with every
request, so use it only behind HTTPS (a reverse proxy) when the page is
reachable from outside.

### `[storage]`

| Key | Default | Description |
|---|---|---|
| `path` | `"watchpost.db"` | SQLite file. A relative path is resolved against the directory of the config file. Missing parent directories are created. |
| `retention_days` | `30` | Individual check results and closed incidents older than this are deleted (hourly). Hourly uptime counters are kept for at least 31 days, so 30-day uptime works even with a shorter retention. |

### `[defaults]`

Defaults for every monitor. Each monitor can override them.

| Key | Default | Description |
|---|---|---|
| `interval` | `"60s"` | Time between checks (minimum `1s`). |
| `timeout` | `"10s"` | Time limit for one check. |
| `retries` | `1` | Extra consecutive failures before the monitor counts as down. With `0`, the first failure is enough. With `1`, it must fail twice in a row. |
| `repeat_reminder` | `"0s"` (off) | While a monitor stays down (or a certificate stays close to expiry), send a reminder this often. Minimum `1m`. |

### `[[monitor]]`: keys shared by all types

| Key | Required | Description |
|---|---|---|
| `name` | yes | Unique name. History is stored under this name, so renaming a monitor starts a new history. |
| `type` | yes | `"http"`, `"tcp"` or `"tls"`. |
| `interval`, `timeout`, `retries`, `repeat_reminder` | no | Override `[defaults]`. |
| `notify` | no | List of notifier names. If omitted, all notifiers are used. `notify = []` turns notifications off for this monitor. |

### `type = "http"`

| Key | Default | Description |
|---|---|---|
| `url` | – | `http://` or `https://` URL. |
| `method` | `"GET"` | Any HTTP method. |
| `expected_status` | `["2xx"]` | Accepted status codes. Entries can be codes (`200`), classes (`"3xx"`) or ranges (`"200-204"`). |
| `keyword` | – | The response body must contain this text. Up to 1 MiB of the body is searched. |
| `headers` | `{}` | Extra request headers, e.g. `{ Authorization = "Bearer ${TOKEN}" }`. |
| `body` | – | Request body (for `POST`/`PUT`). |
| `follow_redirects` | `true` | Follow up to 10 redirects. With `false`, a `3xx` response is checked against `expected_status`. |
| `skip_tls_verify` | `false` | Accept invalid or self-signed certificates. |

Each check uses a new connection, so the latency includes DNS, TCP and TLS
setup, as a new visitor would see it. HTTP monitors honour the standard
`HTTP_PROXY` / `HTTPS_PROXY` / `NO_PROXY` environment variables.

### `type = "tcp"`

| Key | Description |
|---|---|
| `host` | Host name or IP address. |
| `port` | Port. The check passes if a TCP connection can be opened. |

### `type = "tls"`

| Key | Default | Description |
|---|---|---|
| `host` | – | Host to connect to. |
| `port` | `443` | Port. |
| `server_name` | `host` | Name sent as SNI and checked against the certificate. |
| `warn_days` | `14` | Warn (state `warn`, a "certificate expiring" notification) when fewer days than this are left. |
| `skip_verify` | `false` | Only check the expiry date and do not validate the chain or hostname. |
| `ca_file` | – | PEM file with extra trusted CA certificates (for private CAs). |

Results: `up` if the certificate is valid for at least `warn_days`, `warn` if
it expires sooner, and `down` if it has expired, is not trusted (unless
`skip_verify`), or the handshake fails. Certificates do not change often, so
an interval such as `"6h"` is reasonable. The built-in trust store is the
Mozilla root list (via `webpki-roots`) and does not depend on the operating
system.

### `[[notifier]]`

| Key | Required | Description |
|---|---|---|
| `type` | yes | `"telegram"`, `"discord"`, `"webhook"` or `"email"`. |
| `name` | no | Unique name used in `notify = [...]`. Defaults to the type. |

Type-specific keys are listed in the next section.

## Notification setup

Notifications are sent when the state changes:

| Event | When |
|---|---|
| `down` | After `retries + 1` consecutive failed checks. |
| `recovered` | First successful check after being down. Includes how long it was down. |
| `cert_expiring` | A TLS monitor enters the warning window. |
| `reminder` | Only with `repeat_reminder`: still down, or still expiring. |

The first successful check after start-up is silent. A monitor that is down
from the start does notify. A monitor that is still down after a restart is
not announced again (the open incident is stored in the database). Delivery
runs in the background and is retried twice (after 2 s and 10 s). Then the
failure is logged.

Run `watchpost test-notify` after configuring a channel.

### Telegram

1. Talk to [@BotFather](https://t.me/BotFather), send `/newbot` and copy the
   token.
2. Send any message to your new bot (or add it to a group).
3. Find the chat id: open
   `https://api.telegram.org/bot<TOKEN>/getUpdates` and look for
   `"chat":{"id":...}`. Group ids are negative.

```toml
[[notifier]]
name = "telegram"
type = "telegram"
bot_token = "${TELEGRAM_BOT_TOKEN}"
chat_id = "123456789"
# api_url = "https://api.telegram.org"   # for a self-hosted Bot API server
```

Messages are sent as plain text, so names with special characters cannot
break the formatting.

### Discord

In the channel settings, open **Integrations → Webhooks → New Webhook**, then
copy the URL.

```toml
[[notifier]]
name = "discord"
type = "discord"
webhook_url = "${DISCORD_WEBHOOK_URL}"
# username = "watchpost"
```

### Generic webhook

watchpost sends a JSON `POST` to your URL. Any `2xx` answer counts as delivered.

```toml
[[notifier]]
name = "webhook"
type = "webhook"
url = "https://hooks.example.com/watchpost"
headers = { Authorization = "Bearer ${WEBHOOK_TOKEN}" }
```

This is a `recovered` event as it arrived at a local test receiver (pretty-printed; the body is sent as one line with the keys in this order):

```json
{
  "down_since": "2026-11-22T20:54:46Z",
  "duration_seconds": 25,
  "event": "recovered",
  "message": "HTTP 200",
  "monitor": "API",
  "status": "up",
  "status_page": null,
  "subject": "[My services] RECOVERED: API",
  "target": "http://localhost:18082/health",
  "text": "🟢 [My services] RECOVERED: API\n\nTarget: http://localhost:18082/health (http)\nDetail: HTTP 200\nDown for: 25s\nTime: 2026-11-22 20:55:11 UTC",
  "timestamp": "2026-11-22T20:55:11Z",
  "title": "My services",
  "type": "http",
  "unix_time": 1795380911
}
```

`event` is one of `down`, `recovered`, `cert_expiring`, `reminder` and
`test`. `status` is `up`, `warn` or `down`. `down_since` and
`duration_seconds` are `null` when they do not apply. You can use `text` as a
ready-made message, for example for Slack-compatible endpoints that only need
a text field (you may need a small adapter).

### E-mail (SMTP)

```toml
[[notifier]]
name = "email"
type = "email"
host = "smtp.example.com"
tls = "starttls"          # "starttls" (default, port 587), "tls" (port 465) or "none" (port 25)
# port = 587              # defaults to the port for the tls mode
username = "alerts@example.com"
password = "${SMTP_PASSWORD}"
from = "watchpost <alerts@example.com>"
to = ["ops@example.com", "oncall@example.com"]
```

`starttls` requires the server to support STARTTLS; it does not fall back to
plain text. Use `"none"` only for a relay on localhost or a trusted network.
Mails are plain text.

## Status page and API

- `GET /`: the status page (HTML). It reloads every 30 seconds and works
  without JavaScript. A small embedded script only converts timestamps to the
  visitor's local time.
- `GET /api/status`: the same data as JSON. Captured from a local test
  setup and trimmed to one monitor, two history entries and one incident:

  ```json
  {
    "title": "My services",
    "language": "en",
    "generated_at": 1795381001,
    "overall": "down",
    "monitors": [
      {
        "name": "API",
        "type": "http",
        "target": "http://localhost:18082/health",
        "interval_secs": 5,
        "status": "up",
        "since": 1795380911,
        "last_check": 1795381001,
        "last_result": "up",
        "latency_ms": 9,
        "message": "HTTP 200",
        "cert_days_left": null,
        "uptime": {
          "h24": 0.8305084745762712,
          "d7": 0.8305084745762712,
          "d30": 0.8305084745762712
        },
        "history": [
          {
            "ts": 1795380996,
            "status": "up",
            "latency_ms": 25
          },
          {
            "ts": 1795381001,
            "status": "up",
            "latency_ms": 9
          }
        ]
      }
    ],
    "incidents": [
      {
        "monitor": "API",
        "started_at": 1795380886,
        "ended_at": 1795380911,
        "duration_secs": 25,
        "ongoing": false,
        "message": "HTTP 503 (expected 2xx)"
      }
    ]
  }
  ```

  `overall` is `up`, `warn`, `down` or `pending` (no check has finished yet).
  `status` is `null` while a monitor is pending. Timestamps are Unix seconds.
  Uptime values are ratios between 0 and 1, or `null` when there is no data.
  `history` holds the last 60 checks.
- `GET /healthz`: `200 ok` while the process runs. It never requires auth.

**How uptime is calculated:** uptime is the share of checks that were not
`down` (`warn` counts as up). It is based on hourly counters, so a "24 h"
window covers the current hour plus the 23 hours before it.

The page sends a strict `Content-Security-Policy` and loads only its own
embedded CSS, JS and icon.

## Running as a systemd service

An example unit with sandboxing is included: [`watchpost.service`](../watchpost.service).

```sh
sudo install -m 0755 watchpost /usr/local/bin/watchpost
sudo mkdir -p /etc/watchpost
sudo watchpost --config /etc/watchpost/watchpost.toml init
sudo editor /etc/watchpost/watchpost.toml        # set [storage] path = "/var/lib/watchpost/watchpost.db"
# secrets, one per line, e.g. TELEGRAM_BOT_TOKEN=123:abc
sudo install -m 0600 /dev/null /etc/watchpost/watchpost.env && sudo editor /etc/watchpost/watchpost.env
sudo install -m 0644 watchpost.service /etc/systemd/system/watchpost.service
sudo systemctl daemon-reload
sudo systemctl enable --now watchpost
journalctl -u watchpost -f
```

The unit uses `DynamicUser=yes` and `StateDirectory=watchpost`, so watchpost runs as
an unprivileged throw-away user that can write only `/var/lib/watchpost`. After
you change the config, run `watchpost --config /etc/watchpost/watchpost.toml check` and
then `sudo systemctl restart watchpost`.

## Running with Docker

The [`Dockerfile`](../Dockerfile) builds a static binary and copies it into an
empty (`scratch`) image. The image runs as UID 65532 and reads
`/data/watchpost.toml`.

```sh
docker build -t watchpost .

mkdir -p watchpost-data
docker run --rm -v "$PWD/watchpost-data:/data" watchpost --config /data/watchpost.toml init
# Inside a container, listen on all interfaces:
#   [web]
#   listen = "0.0.0.0:8787"
sudo chown -R 65532:65532 watchpost-data     # the container user must be able to write watchpost.db

docker run -d --name watchpost --restart unless-stopped \
  -p 127.0.0.1:8787:8787 \
  -v "$PWD/watchpost-data:/data" \
  -e TELEGRAM_BOT_TOKEN=123:abc \
  watchpost
```

Tip: `listen = "${WATCHPOST_LISTEN:-127.0.0.1:8787}"` in the config together with
`-e WATCHPOST_LISTEN=0.0.0.0:8787` lets the same file work inside and outside a
container. The image has no shell or curl, so check it from the outside, for
example with `curl -f http://127.0.0.1:8787/healthz`.
