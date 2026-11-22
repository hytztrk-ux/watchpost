# Changelog

All notable changes to this project are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-11-22

First public release.

### Added

- Single `watchpost` binary with the commands `init`, `check`, `run` and `test-notify`.
- Configuration in one TOML file, with `${ENV_VAR}` / `${ENV_VAR:-default}`
  substitution, strict unknown-key checking and all validation errors reported
  together.
- Monitors: HTTP(S) (method, headers, body, expected status set, keyword,
  redirects), TCP connect and TLS certificate expiry (with optional private CA).
  Each monitor has its own interval, timeout and retries before it counts as down.
- Alerting on state changes only (down, recovered, certificate expiring), with an
  optional repeat reminder. An open incident survives restarts without being
  announced again.
- Notification channels: Telegram, Discord, generic JSON webhook and SMTP e-mail
  (STARTTLS, implicit TLS or plain). Background delivery with retries.
- SQLite history (bundled, no system library needed) with hourly uptime counters
  and retention pruning.
- Embedded status page (English and Turkish, light and dark, no external assets),
  `/api/status` JSON API, `/healthz` and optional basic auth.
- Graceful shutdown on Ctrl-C / SIGTERM, `--log-level` and `--log-format json`.
- Example systemd unit, multi-stage Dockerfile, CI and release workflows.

[Unreleased]: https://github.com/hytztrk-ux/watchpost/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/hytztrk-ux/watchpost/releases/tag/v0.1.0
