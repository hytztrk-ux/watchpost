use super::*;

fn no_env(_: &str) -> Option<String> {
    None
}

fn parse(text: &str) -> Result<Config, ConfigError> {
    Config::parse(text, &no_env)
}

fn errors_of(text: &str) -> Vec<String> {
    match parse(text) {
        Err(ConfigError::Invalid(v)) => v,
        Err(other) => panic!("expected validation error, got {other}"),
        Ok(_) => panic!("expected an error"),
    }
}

const MINIMAL: &str = r#"
[[monitor]]
name = "site"
type = "http"
url = "http://127.0.0.1:8080/"
"#;

#[test]
fn example_config_is_valid() {
    let cfg = parse(EXAMPLE_CONFIG).expect("example config must parse");
    assert_eq!(cfg.monitors.len(), 3);
    assert_eq!(cfg.language, Language::En);
    assert!(cfg.notifiers.is_empty());
}

#[test]
fn minimal_config_gets_defaults() {
    let cfg = parse(MINIMAL).unwrap();
    assert_eq!(cfg.title, "watchpost");
    assert_eq!(cfg.web.listen, "127.0.0.1:8787");
    assert!(cfg.web.enabled);
    assert_eq!(cfg.storage.retention_days, 30);
    let m = &cfg.monitors[0];
    assert_eq!(m.interval, Duration::from_secs(60));
    assert_eq!(m.timeout, Duration::from_secs(10));
    assert_eq!(m.retries, 1);
    assert_eq!(m.repeat_reminder, None);
    match &m.kind {
        MonitorKind::Http(h) => {
            assert_eq!(h.method, "GET");
            assert_eq!(h.expected_status, vec![StatusMatcher::Range(200, 299)]);
            assert!(h.follow_redirects);
        }
        _ => panic!("expected http"),
    }
}

#[test]
fn full_config_parses() {
    let text = r#"
language = "tr"
title = "Durum"

[web]
listen = "0.0.0.0:9000"
public_url = "https://status.example.com"
[web.auth]
username = "admin"
password = "s3cret"

[storage]
path = "/var/lib/watchpost/history.db"
retention_days = 90

[defaults]
interval = "30s"
timeout = 5
retries = 3
repeat_reminder = "1h"

[[notifier]]
name = "tg"
type = "telegram"
bot_token = "123:abc"
chat_id = -100123

[[notifier]]
type = "discord"
webhook_url = "https://discord.com/api/webhooks/1/x"

[[notifier]]
name = "hook"
type = "webhook"
url = "http://127.0.0.1:9999/hook"
headers = { "X-Token" = "t" }

[[notifier]]
name = "mail"
type = "email"
host = "smtp.example.com"
tls = "tls"
username = "u"
password = "p"
from = "watchpost <alerts@example.com>"
to = "ops@example.com"

[[monitor]]
name = "api"
type = "http"
url = "https://api.example.com/health"
method = "post"
expected_status = [200, "3xx", "401-403"]
keyword = "ok"
headers = { Authorization = "Bearer x" }
body = "{}"
follow_redirects = false
notify = ["tg"]
interval = "2m"

[[monitor]]
name = "db"
type = "tcp"
host = "10.0.0.5"
port = 5432
retries = 0
repeat_reminder = "0s"

[[monitor]]
name = "cert"
type = "tls"
host = "example.com"
warn_days = 21
"#;
    let cfg = parse(text).unwrap();
    assert_eq!(cfg.language, Language::Tr);
    assert_eq!(cfg.title, "Durum");
    assert_eq!(cfg.web.listen_addr().port(), 9000);
    assert_eq!(cfg.web.auth.as_ref().unwrap().username, "admin");
    assert_eq!(cfg.storage.retention_days, 90);
    assert_eq!(cfg.notifiers.len(), 4);
    assert_eq!(cfg.notifiers[1].name, "discord", "name defaults to type");
    match &cfg.notifiers[0].kind {
        NotifierKind::Telegram(t) => assert_eq!(t.chat_id, "-100123"),
        _ => panic!(),
    }
    match &cfg.notifiers[3].kind {
        NotifierKind::Email(e) => {
            assert_eq!(e.to, vec!["ops@example.com".to_string()]);
            assert_eq!(e.effective_port(), 465);
        }
        _ => panic!(),
    }

    let api = &cfg.monitors[0];
    assert_eq!(api.interval, Duration::from_secs(120));
    assert_eq!(api.timeout, Duration::from_secs(5));
    assert_eq!(api.retries, 3);
    assert_eq!(api.repeat_reminder, Some(Duration::from_secs(3600)));
    assert_eq!(api.notify, vec!["tg".to_string()]);
    match &api.kind {
        MonitorKind::Http(h) => {
            assert!(h.expected_status.iter().any(|m| m.matches(200)));
            assert!(h.expected_status.iter().any(|m| m.matches(302)));
            assert!(h.expected_status.iter().any(|m| m.matches(402)));
            assert!(!h.expected_status.iter().any(|m| m.matches(500)));
        }
        _ => panic!(),
    }

    let db = &cfg.monitors[1];
    assert_eq!(db.retries, 0);
    assert_eq!(db.repeat_reminder, None, "0s disables reminders");
    assert_eq!(db.notify.len(), 4, "no notify key = all notifiers");
    assert_eq!(db.kind.target(), "10.0.0.5:5432");

    match &cfg.monitors[2].kind {
        MonitorKind::Tls(t) => {
            assert_eq!(t.port, 443);
            assert_eq!(t.warn_days, 21);
        }
        _ => panic!(),
    }
}

#[test]
fn env_substitution_in_secrets() {
    let text = r#"
[[notifier]]
name = "tg"
type = "telegram"
bot_token = "${TG_TOKEN}"
chat_id = "${TG_CHAT:-42}"

[[monitor]]
name = "site"
type = "http"
url = "http://${HOST}:8080/"
"#;
    let env = |k: &str| match k {
        "TG_TOKEN" => Some("999:secret".to_string()),
        "HOST" => Some("10.1.2.3".to_string()),
        _ => None,
    };
    let cfg = Config::parse(text, &env).unwrap();
    match &cfg.notifiers[0].kind {
        NotifierKind::Telegram(t) => {
            assert_eq!(t.bot_token, "999:secret");
            assert_eq!(t.chat_id, "42");
        }
        _ => panic!(),
    }
    assert_eq!(cfg.monitors[0].kind.target(), "http://10.1.2.3:8080/");
}

#[test]
fn env_references_in_comments_are_ignored() {
    let text = format!("# token = \"${{NOT_SET_ANYWHERE}}\"\n{MINIMAL}");
    assert!(parse(&text).is_ok());
}

#[test]
fn missing_env_var_is_reported_with_path() {
    let text = r#"
[[notifier]]
type = "discord"
webhook_url = "${DISCORD_URL}"

[[monitor]]
name = "site"
type = "tcp"
host = "localhost"
port = 1
"#;
    let err = parse(text).unwrap_err().to_string();
    assert!(err.contains("DISCORD_URL"), "{err}");
    assert!(err.contains("notifier[0].webhook_url"), "{err}");
}

#[test]
fn debug_output_redacts_secrets() {
    let text = r#"
[web.auth]
username = "admin"
password = "hunter2"

[[notifier]]
type = "telegram"
bot_token = "123:topsecret"
chat_id = "1"

[[monitor]]
name = "site"
type = "tcp"
host = "localhost"
port = 1
"#;
    let cfg = parse(text).unwrap();
    let dbg = format!("{cfg:?}");
    assert!(!dbg.contains("hunter2"), "{dbg}");
    assert!(!dbg.contains("topsecret"), "{dbg}");
}

#[test]
fn syntax_error_is_reported() {
    let err = parse("[[monitor]\nname = ").unwrap_err();
    assert!(matches!(err, ConfigError::Syntax(_)), "{err}");
}

#[test]
fn unknown_keys_are_rejected() {
    let errs = errors_of(&format!("titel = \"typo\"\n{MINIMAL}"));
    assert!(errs[0].contains("titel"), "{errs:?}");

    let errs = errors_of(
        r#"
[[monitor]]
name = "site"
type = "http"
url = "http://localhost/"
keywrod = "x"
"#,
    );
    assert!(errs[0].contains("monitor \"site\""), "{errs:?}");
    assert!(errs[0].contains("keywrod"), "{errs:?}");
}

#[test]
fn type_specific_keys_are_not_shared() {
    // `port` is a tcp key; it must not be accepted on an http monitor.
    let errs = errors_of(
        r#"
[[monitor]]
name = "site"
type = "http"
url = "http://localhost/"
port = 80
"#,
    );
    assert!(errs[0].contains("port"), "{errs:?}");
}

#[test]
fn unknown_types_are_rejected() {
    let errs = errors_of(
        r#"
[[monitor]]
name = "p"
type = "ping"
host = "x"
"#,
    );
    assert!(errs[0].contains("unknown type `ping`"), "{errs:?}");
}

#[test]
fn many_problems_are_reported_together() {
    let errs = errors_of(
        r#"
language = "en"
[web]
listen = "not-an-address"
[storage]
retention_days = 0

[[notifier]]
name = "hook"
type = "webhook"
url = "ftp://example.com"

[[notifier]]
name = "hook"
type = "email"
host = "smtp"
from = "not an email"
to = []

[[monitor]]
name = "a"
type = "http"
url = "localhost"
expected_status = [700]
notify = ["nope"]
interval = "100ms"

[[monitor]]
name = "a"
type = "tcp"
host = ""
port = 0

[[monitor]]
name = "c"
type = "tls"
host = "example.com"
warn_days = 0
repeat_reminder = "10s"
"#,
    );
    let all = errs.join("\n");
    for needle in [
        "web.listen",
        "retention_days",
        "ftp://example.com",
        "duplicate notifier name",
        "from `not an email`",
        "at least one address",
        "url `localhost`",
        "outside 100-599",
        "unknown notifier `nope`",
        "interval must be at least 1s",
        "duplicate monitor name",
        "host must not be empty",
        "port must not be 0",
        "warn_days",
        "repeat_reminder",
    ] {
        assert!(all.contains(needle), "missing `{needle}` in:\n{all}");
    }
}

#[test]
fn bad_values_are_rejected() {
    let errs = errors_of(
        r#"
[[monitor]]
name = "a"
type = "http"
url = "http://localhost/"
interval = "soon"
"#,
    );
    assert!(errs[0].contains("invalid duration"), "{errs:?}");

    let errs = errors_of(
        r#"
[[monitor]]
name = "a"
type = "http"
url = "http://localhost/"
expected_status = ["9xx"]
"#,
    );
    assert!(errs[0].contains("invalid status"), "{errs:?}");

    let errs =
        errors_of("language = \"de\"\n[[monitor]]\nname=\"a\"\ntype=\"tcp\"\nhost=\"h\"\nport=1\n");
    assert!(errs[0].contains("de"), "{errs:?}");
}

#[test]
fn no_monitors_is_an_error() {
    let errs = errors_of("title = \"x\"\n");
    assert!(errs[0].contains("no monitors"), "{errs:?}");
}

#[test]
fn notify_empty_list_means_none() {
    let cfg = parse(
        r#"
[[notifier]]
type = "webhook"
url = "http://127.0.0.1:1/"

[[monitor]]
name = "quiet"
type = "tcp"
host = "localhost"
port = 1
notify = []
"#,
    )
    .unwrap();
    assert!(cfg.monitors[0].notify.is_empty());
}

#[test]
fn status_matcher_display_roundtrip() {
    assert_eq!(StatusMatcher::parse("2xx").unwrap().to_string(), "2xx");
    assert_eq!(
        StatusMatcher::parse("200-204").unwrap().to_string(),
        "200-204"
    );
    assert_eq!(StatusMatcher::parse("418").unwrap().to_string(), "418");
    assert!(StatusMatcher::parse("300-200").is_err());
}

#[test]
fn relative_db_path_is_resolved_next_to_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watchpost.toml");
    std::fs::write(&path, MINIMAL).unwrap();
    let cfg = Config::load(&path).unwrap();
    assert_eq!(cfg.storage.path, dir.path().join("watchpost.db"));
}

#[test]
fn missing_file_is_a_read_error() {
    let err = Config::load(Path::new("/definitely/not/here/watchpost.toml")).unwrap_err();
    assert!(matches!(err, ConfigError::Read { .. }));
}
