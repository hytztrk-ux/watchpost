# Contributing to watchpost

Thank you for your interest in watchpost. Bug reports, ideas and pull requests are
welcome, in English or Turkish.

## Reporting bugs

Please open an issue with:

- the watchpost version (`watchpost --version`) and your operating system,
- the relevant part of your `watchpost.toml` (**remove tokens and passwords**),
- what you expected and what happened, ideally with logs from
  `watchpost --log-level debug run`.

## Development

You need a stable Rust toolchain (1.88 or newer). No other system libraries are
required: SQLite is bundled and TLS uses rustls.

```sh
cargo build
cargo test
cargo fmt --all
cargo clippy --all-targets -- -D warnings
```

CI runs exactly these checks on Linux, Windows and macOS, so please run them
before opening a pull request.

### Tests

- Tests must not need the internet. Use local listeners on `127.0.0.1` with
  ephemeral ports (see `tests/common/mod.rs` for helpers such as `Target` and
  `Recorder`).
- Logic that decides *when* to alert lives in `src/alert.rs` and is tested
  without any I/O. Please keep it that way.
- New notification channels need a test against a local mock server.

### Project layout

| Path | Contents |
|---|---|
| `src/config/` | TOML loading, `${ENV}` substitution, validation |
| `src/monitor/` | HTTP, TCP and TLS probes |
| `src/alert.rs` | State machine: retries, transitions, reminders |
| `src/scheduler.rs` | Runs probes, records results, dispatches events |
| `src/store.rs` | SQLite history, uptime, incidents, pruning |
| `src/notify/` | Message rendering and channels |
| `src/web/` | Status page, JSON API, embedded assets |
| `src/i18n.rs` | English and Turkish texts |
| `tests/` | Integration tests |

### Guidelines

- Keep the binary small and the memory footprint low. Discuss new dependencies
  in an issue first.
- No external assets on the status page (no CDN, no fonts, no JS frameworks).
- User-visible text on the status page and in notifications must exist in both
  English and Turkish (`src/i18n.rs`).
- Update `README.md`, `README.tr.md`, `docs/` and `CHANGELOG.md` when behaviour or
  configuration changes.

## License

By contributing, you agree that your contributions are licensed under the MIT
license of this project.
