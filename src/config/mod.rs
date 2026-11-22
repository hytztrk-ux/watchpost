//! Configuration: loading `watchpost.toml`, `${ENV}` substitution, defaults and
//! validation.
//!
//! Loading happens in three steps:
//!
//! 1. the file is parsed as TOML (syntax errors carry line numbers),
//! 2. `${VAR}` references in string values are substituted,
//! 3. the tree is converted into typed structures and validated. All
//!    validation problems are collected and reported together.

mod env;

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Deserializer};

pub use env::{substitute, EnvLookup};

/// The commented example configuration written by `watchpost init`.
pub const EXAMPLE_CONFIG: &str = include_str!("../../watchpost.example.toml");

/// Errors produced while loading a configuration file.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read config file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid TOML: {0}")]
    Syntax(#[from] toml::de::Error),
    #[error("in `{path}`: {message}")]
    Env { path: String, message: String },
    #[error("invalid configuration:\n{}", format_list(.0))]
    Invalid(Vec<String>),
}

fn format_list(items: &[String]) -> String {
    items
        .iter()
        .map(|s| format!("  - {s}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Status page / notification language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    En,
    Tr,
}

/// Fully loaded and validated configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub language: Language,
    pub title: String,
    pub web: WebConfig,
    pub storage: StorageConfig,
    pub notifiers: Vec<NotifierConfig>,
    pub monitors: Vec<Monitor>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_listen")]
    pub listen: String,
    /// Public URL of the status page, included in notifications if set.
    #[serde(default)]
    pub public_url: Option<String>,
    #[serde(default)]
    pub auth: Option<AuthConfig>,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            listen: default_listen(),
            public_url: None,
            auth: None,
        }
    }
}

impl WebConfig {
    /// The listen address. Only valid after validation succeeded.
    pub fn listen_addr(&self) -> SocketAddr {
        self.listen
            .parse()
            .expect("listen address is validated at load time")
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthConfig {
    pub username: String,
    pub password: String,
}

impl fmt::Debug for AuthConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthConfig")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageConfig {
    /// Path of the SQLite database. Relative paths are resolved against the
    /// directory containing the config file.
    #[serde(default = "default_db_path")]
    pub path: PathBuf,
    #[serde(default = "default_retention")]
    pub retention_days: u32,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            path: default_db_path(),
            retention_days: default_retention(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Defaults {
    #[serde(default = "default_interval")]
    interval: Dur,
    #[serde(default = "default_timeout")]
    timeout: Dur,
    #[serde(default = "default_retries")]
    retries: u32,
    #[serde(default)]
    repeat_reminder: Option<Dur>,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            interval: default_interval(),
            timeout: default_timeout(),
            retries: default_retries(),
            repeat_reminder: None,
        }
    }
}

/// A monitor with all defaults applied.
#[derive(Debug, Clone)]
pub struct Monitor {
    pub name: String,
    pub interval: Duration,
    pub timeout: Duration,
    /// Number of additional consecutive failures tolerated before the monitor
    /// is considered down. `0` means the first failure marks it down.
    pub retries: u32,
    /// Re-send the "still down" notification this often. `None` disables it.
    pub repeat_reminder: Option<Duration>,
    /// Names of the notifiers this monitor reports to.
    pub notify: Vec<String>,
    pub kind: MonitorKind,
}

#[derive(Debug, Clone)]
pub enum MonitorKind {
    Http(HttpMonitor),
    Tcp(TcpMonitor),
    Tls(TlsMonitor),
}

impl MonitorKind {
    pub fn type_name(&self) -> &'static str {
        match self {
            MonitorKind::Http(_) => "http",
            MonitorKind::Tcp(_) => "tcp",
            MonitorKind::Tls(_) => "tls",
        }
    }

    /// Human readable target (URL or host:port).
    pub fn target(&self) -> String {
        match self {
            MonitorKind::Http(h) => h.url.clone(),
            MonitorKind::Tcp(t) => format!("{}:{}", t.host, t.port),
            MonitorKind::Tls(t) => format!("{}:{}", t.host, t.port),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpMonitor {
    pub url: String,
    #[serde(default = "default_method")]
    pub method: String,
    #[serde(default = "default_expected_status")]
    pub expected_status: Vec<StatusMatcher>,
    /// The response body must contain this text.
    #[serde(default)]
    pub keyword: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default = "default_true")]
    pub follow_redirects: bool,
    /// Accept invalid / self-signed certificates.
    #[serde(default)]
    pub skip_tls_verify: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TcpMonitor {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TlsMonitor {
    pub host: String,
    #[serde(default = "default_tls_port")]
    pub port: u16,
    /// SNI / certificate name to check; defaults to `host`.
    #[serde(default)]
    pub server_name: Option<String>,
    #[serde(default = "default_warn_days")]
    pub warn_days: u32,
    /// Only check the expiry date, do not validate the certificate chain.
    #[serde(default)]
    pub skip_verify: bool,
    /// Extra PEM file with trusted CA certificates (for private CAs).
    #[serde(default)]
    pub ca_file: Option<PathBuf>,
}

/// One entry of `expected_status`: a code (`200`), a class (`"2xx"`) or an
/// inclusive range (`"200-204"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusMatcher {
    Code(u16),
    Range(u16, u16),
}

impl StatusMatcher {
    pub fn matches(&self, code: u16) -> bool {
        match *self {
            StatusMatcher::Code(c) => c == code,
            StatusMatcher::Range(lo, hi) => (lo..=hi).contains(&code),
        }
    }

    fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let bad = || format!("invalid status `{s}` (use 200, \"2xx\" or \"200-299\")");
        if s.len() == 3 && s.ends_with("xx") {
            let d = s[..1].parse::<u16>().map_err(|_| bad())?;
            if !(1..=5).contains(&d) {
                return Err(bad());
            }
            return Ok(StatusMatcher::Range(d * 100, d * 100 + 99));
        }
        if let Some((a, b)) = s.split_once('-') {
            let a = a.trim().parse::<u16>().map_err(|_| bad())?;
            let b = b.trim().parse::<u16>().map_err(|_| bad())?;
            if a > b {
                return Err(bad());
            }
            return Ok(StatusMatcher::Range(a, b));
        }
        s.parse::<u16>().map(StatusMatcher::Code).map_err(|_| bad())
    }

    fn in_bounds(&self) -> bool {
        let ok = |c: u16| (100..=599).contains(&c);
        match *self {
            StatusMatcher::Code(c) => ok(c),
            StatusMatcher::Range(a, b) => ok(a) && ok(b),
        }
    }
}

impl fmt::Display for StatusMatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            StatusMatcher::Code(c) => write!(f, "{c}"),
            StatusMatcher::Range(a, b) if a % 100 == 0 && b == a + 99 => {
                write!(f, "{}xx", a / 100)
            }
            StatusMatcher::Range(a, b) => write!(f, "{a}-{b}"),
        }
    }
}

impl<'de> Deserialize<'de> for StatusMatcher {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Int(i64),
            Str(String),
        }
        match Raw::deserialize(d)? {
            Raw::Int(i) => u16::try_from(i)
                .map(StatusMatcher::Code)
                .map_err(|_| serde::de::Error::custom(format!("invalid status code {i}"))),
            Raw::Str(s) => StatusMatcher::parse(&s).map_err(serde::de::Error::custom),
        }
    }
}

/// A notification channel.
#[derive(Debug, Clone)]
pub struct NotifierConfig {
    pub name: String,
    pub kind: NotifierKind,
}

#[derive(Debug, Clone)]
pub enum NotifierKind {
    Telegram(TelegramConfig),
    Discord(DiscordConfig),
    Webhook(WebhookConfig),
    Email(EmailConfig),
}

impl NotifierKind {
    pub fn type_name(&self) -> &'static str {
        match self {
            NotifierKind::Telegram(_) => "telegram",
            NotifierKind::Discord(_) => "discord",
            NotifierKind::Webhook(_) => "webhook",
            NotifierKind::Email(_) => "email",
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelegramConfig {
    pub bot_token: String,
    #[serde(deserialize_with = "string_or_int")]
    pub chat_id: String,
    /// Base URL of the Bot API (override for a local Bot API server).
    #[serde(default = "default_telegram_api")]
    pub api_url: String,
}

impl fmt::Debug for TelegramConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TelegramConfig")
            .field("bot_token", &"<redacted>")
            .field("chat_id", &self.chat_id)
            .field("api_url", &self.api_url)
            .finish()
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscordConfig {
    pub webhook_url: String,
    #[serde(default)]
    pub username: Option<String>,
}

impl fmt::Debug for DiscordConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DiscordConfig")
            .field("webhook_url", &"<redacted>")
            .field("username", &self.username)
            .finish()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookConfig {
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SmtpTls {
    /// Plain connection upgraded with STARTTLS (port 587). Required.
    #[default]
    Starttls,
    /// Implicit TLS (port 465).
    Tls,
    /// No encryption at all. Only for local relays.
    None,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmailConfig {
    pub host: String,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub tls: SmtpTls,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    pub from: String,
    #[serde(deserialize_with = "string_or_vec")]
    pub to: Vec<String>,
}

impl EmailConfig {
    pub fn effective_port(&self) -> u16 {
        self.port.unwrap_or(match self.tls {
            SmtpTls::Starttls => 587,
            SmtpTls::Tls => 465,
            SmtpTls::None => 25,
        })
    }
}

impl fmt::Debug for EmailConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmailConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("tls", &self.tls)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("from", &self.from)
            .field("to", &self.to)
            .finish()
    }
}

/// Duration accepting `"30s"`, `"5m"`, `"1h 30m"` or a plain number of seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Dur(Duration);

impl<'de> Deserialize<'de> for Dur {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Int(i64),
            Str(String),
        }
        match Raw::deserialize(d)? {
            Raw::Int(i) if i >= 0 => Ok(Dur(Duration::from_secs(i as u64))),
            Raw::Int(i) => Err(serde::de::Error::custom(format!(
                "duration must not be negative (got {i})"
            ))),
            Raw::Str(s) => humantime::parse_duration(s.trim()).map(Dur).map_err(|e| {
                serde::de::Error::custom(format!(
                    "invalid duration `{s}` ({e}); use e.g. \"30s\", \"5m\", \"1h\""
                ))
            }),
        }
    }
}

fn string_or_int<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Int(i64),
        Str(String),
    }
    Ok(match Raw::deserialize(d)? {
        Raw::Int(i) => i.to_string(),
        Raw::Str(s) => s,
    })
}

fn string_or_vec<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        One(String),
        Many(Vec<String>),
    }
    Ok(match Raw::deserialize(d)? {
        Raw::One(s) => vec![s],
        Raw::Many(v) => v,
    })
}

fn default_true() -> bool {
    true
}
fn default_listen() -> String {
    "127.0.0.1:8787".to_owned()
}
fn default_db_path() -> PathBuf {
    PathBuf::from("watchpost.db")
}
fn default_retention() -> u32 {
    30
}
fn default_interval() -> Dur {
    Dur(Duration::from_secs(60))
}
fn default_timeout() -> Dur {
    Dur(Duration::from_secs(10))
}
fn default_retries() -> u32 {
    1
}
fn default_method() -> String {
    "GET".to_owned()
}
fn default_expected_status() -> Vec<StatusMatcher> {
    vec![StatusMatcher::Range(200, 299)]
}
fn default_tls_port() -> u16 {
    443
}
fn default_warn_days() -> u32 {
    14
}
fn default_telegram_api() -> String {
    "https://api.telegram.org".to_owned()
}
fn default_title() -> String {
    "watchpost".to_owned()
}

/// Top-level shape of the file. Monitors and notifiers are kept as raw tables
/// because their accepted keys depend on their `type`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    language: Language,
    #[serde(default = "default_title")]
    title: String,
    #[serde(default)]
    web: WebConfig,
    #[serde(default)]
    storage: StorageConfig,
    #[serde(default)]
    defaults: Defaults,
    #[serde(default)]
    notifier: Vec<toml::Table>,
    #[serde(default)]
    monitor: Vec<toml::Table>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MonitorCommon {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    interval: Option<Dur>,
    #[serde(default)]
    timeout: Option<Dur>,
    #[serde(default)]
    retries: Option<u32>,
    #[serde(default)]
    repeat_reminder: Option<Dur>,
    #[serde(default)]
    notify: Option<Vec<String>>,
}

const MONITOR_COMMON_KEYS: &[&str] = &[
    "name",
    "type",
    "interval",
    "timeout",
    "retries",
    "repeat_reminder",
    "notify",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NotifierCommon {
    #[serde(default)]
    name: Option<String>,
    #[serde(rename = "type")]
    kind: String,
}

const NOTIFIER_COMMON_KEYS: &[&str] = &["name", "type"];

/// Split a table into (common keys, type-specific keys).
fn split_table(table: toml::Table, common: &[&str]) -> (toml::Table, toml::Table) {
    let mut a = toml::Table::new();
    let mut b = toml::Table::new();
    for (k, v) in table {
        if common.contains(&k.as_str()) {
            a.insert(k, v);
        } else {
            b.insert(k, v);
        }
    }
    (a, b)
}

fn clean_serde_error(e: toml::de::Error) -> String {
    e.message().trim().to_owned()
}

impl Config {
    /// Load the configuration from a file using the process environment for
    /// `${VAR}` substitution.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        let mut cfg = Config::parse(&text, &|k| std::env::var(k).ok())?;
        if cfg.storage.path.is_relative() {
            if let Some(dir) = path.parent() {
                cfg.storage.path = dir.join(&cfg.storage.path);
            }
        }
        Ok(cfg)
    }

    /// Parse and validate configuration text.
    pub fn parse(text: &str, env: EnvLookup<'_>) -> Result<Config, ConfigError> {
        let mut value: toml::Value = toml::Value::Table(text.parse::<toml::Table>()?);
        env::substitute_value(&mut value, "", env)?;
        let raw = RawConfig::deserialize(value)
            .map_err(|e| ConfigError::Invalid(vec![clean_serde_error(e)]))?;

        let mut errors = Vec::new();

        let mut notifiers = Vec::new();
        for (i, table) in raw.notifier.into_iter().enumerate() {
            let label = table
                .get("name")
                .and_then(|v| v.as_str())
                .map(|n| format!("notifier \"{n}\""))
                .unwrap_or_else(|| format!("notifier #{}", i + 1));
            let (common, specific) = split_table(table, NOTIFIER_COMMON_KEYS);
            let common = match NotifierCommon::deserialize(common) {
                Ok(c) => c,
                Err(e) => {
                    errors.push(format!("{label}: {}", clean_serde_error(e)));
                    continue;
                }
            };
            let kind = match common.kind.as_str() {
                "telegram" => TelegramConfig::deserialize(specific).map(NotifierKind::Telegram),
                "discord" => DiscordConfig::deserialize(specific).map(NotifierKind::Discord),
                "webhook" => WebhookConfig::deserialize(specific).map(NotifierKind::Webhook),
                "email" => EmailConfig::deserialize(specific).map(NotifierKind::Email),
                other => {
                    errors.push(format!(
                        "{label}: unknown type `{other}` (expected telegram, discord, webhook or email)"
                    ));
                    continue;
                }
            };
            match kind {
                Ok(kind) => notifiers.push(NotifierConfig {
                    name: common.name.unwrap_or_else(|| common.kind.clone()),
                    kind,
                }),
                Err(e) => errors.push(format!("{label}: {}", clean_serde_error(e))),
            }
        }

        let all_notifiers: Vec<String> = notifiers.iter().map(|n| n.name.clone()).collect();
        let defaults = &raw.defaults;
        let mut monitors = Vec::new();
        for (i, table) in raw.monitor.into_iter().enumerate() {
            let label = table
                .get("name")
                .and_then(|v| v.as_str())
                .map(|n| format!("monitor \"{n}\""))
                .unwrap_or_else(|| format!("monitor #{}", i + 1));
            let (common, specific) = split_table(table, MONITOR_COMMON_KEYS);
            let common = match MonitorCommon::deserialize(common) {
                Ok(c) => c,
                Err(e) => {
                    errors.push(format!("{label}: {}", clean_serde_error(e)));
                    continue;
                }
            };
            let kind = match common.kind.as_str() {
                "http" => HttpMonitor::deserialize(specific).map(MonitorKind::Http),
                "tcp" => TcpMonitor::deserialize(specific).map(MonitorKind::Tcp),
                "tls" => TlsMonitor::deserialize(specific).map(MonitorKind::Tls),
                other => {
                    errors.push(format!(
                        "{label}: unknown type `{other}` (expected http, tcp or tls)"
                    ));
                    continue;
                }
            };
            let kind = match kind {
                Ok(k) => k,
                Err(e) => {
                    errors.push(format!("{label}: {}", clean_serde_error(e)));
                    continue;
                }
            };
            let repeat = common
                .repeat_reminder
                .or(defaults.repeat_reminder)
                .map(|d| d.0)
                .filter(|d| !d.is_zero());
            monitors.push(Monitor {
                name: common.name,
                interval: common.interval.unwrap_or(defaults.interval).0,
                timeout: common.timeout.unwrap_or(defaults.timeout).0,
                retries: common.retries.unwrap_or(defaults.retries),
                repeat_reminder: repeat,
                notify: common.notify.unwrap_or_else(|| all_notifiers.clone()),
                kind,
            });
        }

        let cfg = Config {
            language: raw.language,
            title: raw.title,
            web: raw.web,
            storage: raw.storage,
            notifiers,
            monitors,
        };
        cfg.validate(&mut errors);
        if errors.is_empty() {
            Ok(cfg)
        } else {
            Err(ConfigError::Invalid(errors))
        }
    }

    fn validate(&self, errors: &mut Vec<String>) {
        if self.title.trim().is_empty() {
            errors.push("title must not be empty".into());
        }
        if self.web.listen.parse::<SocketAddr>().is_err() {
            errors.push(format!(
                "web.listen `{}` is not a valid address (expected e.g. \"127.0.0.1:8787\")",
                self.web.listen
            ));
        }
        if let Some(url) = &self.web.public_url {
            check_http_url(url, "web.public_url", errors);
        }
        if let Some(auth) = &self.web.auth {
            if auth.username.is_empty() || auth.password.is_empty() {
                errors.push("web.auth: username and password must not be empty".into());
            }
            if auth.username.contains(':') {
                errors.push("web.auth: username must not contain ':'".into());
            }
        }
        if self.storage.retention_days == 0 {
            errors.push("storage.retention_days must be at least 1".into());
        }

        let mut names = HashSet::new();
        for n in &self.notifiers {
            let label = format!("notifier \"{}\"", n.name);
            if n.name.trim().is_empty() {
                errors.push("notifier name must not be empty".into());
            }
            if !names.insert(n.name.as_str()) {
                errors.push(format!("{label}: duplicate notifier name"));
            }
            match &n.kind {
                NotifierKind::Telegram(t) => {
                    if t.bot_token.trim().is_empty() {
                        errors.push(format!("{label}: bot_token must not be empty"));
                    }
                    if t.chat_id.trim().is_empty() {
                        errors.push(format!("{label}: chat_id must not be empty"));
                    }
                    check_http_url(&t.api_url, &format!("{label}: api_url"), errors);
                }
                NotifierKind::Discord(d) => {
                    check_http_url(&d.webhook_url, &format!("{label}: webhook_url"), errors)
                }
                NotifierKind::Webhook(w) => {
                    check_http_url(&w.url, &format!("{label}: url"), errors);
                    check_headers(&w.headers, &label, errors);
                }
                NotifierKind::Email(e) => {
                    if e.host.trim().is_empty() {
                        errors.push(format!("{label}: host must not be empty"));
                    }
                    if e.from.parse::<lettre::message::Mailbox>().is_err() {
                        errors.push(format!("{label}: from `{}` is not a valid address", e.from));
                    }
                    if e.to.is_empty() {
                        errors.push(format!("{label}: `to` needs at least one address"));
                    }
                    for to in &e.to {
                        if to.parse::<lettre::message::Mailbox>().is_err() {
                            errors.push(format!("{label}: to `{to}` is not a valid address"));
                        }
                    }
                    if e.username.is_some() != e.password.is_some() {
                        errors.push(format!(
                            "{label}: username and password must be given together"
                        ));
                    }
                }
            }
        }

        if self.monitors.is_empty() {
            errors.push("no monitors defined (add at least one [[monitor]])".into());
        }
        let mut mnames = HashSet::new();
        for m in &self.monitors {
            let label = format!("monitor \"{}\"", m.name);
            if m.name.trim().is_empty() {
                errors.push("monitor name must not be empty".into());
            }
            if !mnames.insert(m.name.as_str()) {
                errors.push(format!("{label}: duplicate monitor name"));
            }
            if m.interval < Duration::from_secs(1) {
                errors.push(format!("{label}: interval must be at least 1s"));
            }
            if m.timeout.is_zero() {
                errors.push(format!("{label}: timeout must be greater than 0"));
            }
            if m.retries > 100 {
                errors.push(format!("{label}: retries must be at most 100"));
            }
            if let Some(r) = m.repeat_reminder {
                if r < Duration::from_secs(60) {
                    errors.push(format!("{label}: repeat_reminder must be at least 1m"));
                }
            }
            for n in &m.notify {
                if !names.contains(n.as_str()) {
                    errors.push(format!("{label}: unknown notifier `{n}` in notify"));
                }
            }
            match &m.kind {
                MonitorKind::Http(h) => {
                    check_http_url(&h.url, &format!("{label}: url"), errors);
                    if reqwest::Method::from_bytes(h.method.to_ascii_uppercase().as_bytes())
                        .is_err()
                    {
                        errors.push(format!("{label}: invalid method `{}`", h.method));
                    }
                    if h.expected_status.is_empty() {
                        errors.push(format!("{label}: expected_status must not be empty"));
                    }
                    for s in &h.expected_status {
                        if !s.in_bounds() {
                            errors
                                .push(format!("{label}: expected_status `{s}` is outside 100-599"));
                        }
                    }
                    if matches!(&h.keyword, Some(k) if k.is_empty()) {
                        errors.push(format!("{label}: keyword must not be empty"));
                    }
                    check_headers(&h.headers, &label, errors);
                }
                MonitorKind::Tcp(t) => {
                    if t.host.trim().is_empty() {
                        errors.push(format!("{label}: host must not be empty"));
                    }
                    if t.port == 0 {
                        errors.push(format!("{label}: port must not be 0"));
                    }
                }
                MonitorKind::Tls(t) => {
                    if t.host.trim().is_empty() {
                        errors.push(format!("{label}: host must not be empty"));
                    }
                    if t.port == 0 {
                        errors.push(format!("{label}: port must not be 0"));
                    }
                    if t.warn_days == 0 {
                        errors.push(format!("{label}: warn_days must be at least 1"));
                    }
                    let sni = t.server_name.as_deref().unwrap_or(&t.host);
                    if rustls::pki_types::ServerName::try_from(sni.to_owned()).is_err() {
                        errors.push(format!("{label}: `{sni}` is not a valid server name"));
                    }
                }
            }
        }
    }
}

fn check_http_url(url: &str, what: &str, errors: &mut Vec<String>) {
    match reqwest::Url::parse(url) {
        Ok(u) if (u.scheme() == "http" || u.scheme() == "https") && u.host().is_some() => {}
        Ok(_) => errors.push(format!("{what} `{url}` must be an http:// or https:// URL")),
        Err(e) => errors.push(format!("{what} `{url}` is not a valid URL ({e})")),
    }
}

fn check_headers(headers: &BTreeMap<String, String>, label: &str, errors: &mut Vec<String>) {
    for (k, v) in headers {
        if reqwest::header::HeaderName::from_bytes(k.as_bytes()).is_err() {
            errors.push(format!("{label}: invalid header name `{k}`"));
        }
        if reqwest::header::HeaderValue::from_str(v).is_err() {
            errors.push(format!("{label}: invalid value for header `{k}`"));
        }
    }
}

#[cfg(test)]
mod tests;
