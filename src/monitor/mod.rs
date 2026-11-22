//! Probes: run a single check against a target and report the outcome.

mod http;
mod tcp;
mod tls;

use std::error::Error as _;
use std::fmt;
use std::time::Duration;

use serde::Serialize;

use crate::config::{Monitor, MonitorKind};

/// Result of a single check (or the confirmed state of a monitor).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Up,
    /// Reachable, but needs attention (e.g. certificate about to expire).
    /// Counts as "up" for uptime.
    Warn,
    Down,
}

impl Status {
    pub fn as_i64(self) -> i64 {
        match self {
            Status::Down => 0,
            Status::Up => 1,
            Status::Warn => 2,
        }
    }

    pub fn from_i64(v: i64) -> Status {
        match v {
            1 => Status::Up,
            2 => Status::Warn,
            _ => Status::Down,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Up => "up",
            Status::Warn => "warn",
            Status::Down => "down",
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Outcome of one check.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckResult {
    pub status: Status,
    /// Time until the check completed (response, connect or handshake).
    pub latency: Option<Duration>,
    /// Short human readable detail ("HTTP 200", "connection refused", ...).
    pub message: String,
    /// Days until certificate expiry (TLS monitors only).
    pub cert_days_left: Option<i64>,
}

impl CheckResult {
    pub fn up(latency: Duration, message: impl Into<String>) -> Self {
        Self {
            status: Status::Up,
            latency: Some(latency),
            message: message.into(),
            cert_days_left: None,
        }
    }

    pub fn down(latency: Option<Duration>, message: impl Into<String>) -> Self {
        Self {
            status: Status::Down,
            latency,
            message: message.into(),
            cert_days_left: None,
        }
    }
}

/// Errors that prevent a probe from being created at all.
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("monitor \"{monitor}\": cannot build HTTP client: {source}")]
    HttpClient {
        monitor: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("monitor \"{monitor}\": cannot load ca_file {path}: {message}")]
    CaFile {
        monitor: String,
        path: String,
        message: String,
    },
    #[error("monitor \"{monitor}\": {message}")]
    Tls { monitor: String, message: String },
}

/// A ready-to-run probe for one monitor. Created once, reused for every check.
pub enum Probe {
    Http(Box<http::HttpProbe>),
    Tcp(tcp::TcpProbe),
    Tls(tls::TlsProbe),
}

impl Probe {
    pub fn new(monitor: &Monitor) -> Result<Probe, ProbeError> {
        Ok(match &monitor.kind {
            MonitorKind::Http(cfg) => Probe::Http(Box::new(http::HttpProbe::new(monitor, cfg)?)),
            MonitorKind::Tcp(cfg) => Probe::Tcp(tcp::TcpProbe::new(monitor, cfg)),
            MonitorKind::Tls(cfg) => Probe::Tls(tls::TlsProbe::new(monitor, cfg)?),
        })
    }

    /// Run one check. Never panics and never exceeds the monitor timeout by
    /// more than a few milliseconds.
    pub async fn check(&self) -> CheckResult {
        match self {
            Probe::Http(p) => p.check().await,
            Probe::Tcp(p) => p.check().await,
            Probe::Tls(p) => p.check().await,
        }
    }
}

/// Render an error and its sources compactly, skipping repeated text.
pub(crate) fn error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut parts: Vec<String> = vec![err.to_string()];
    let mut source = err.source();
    while let Some(e) = source {
        let s = e.to_string();
        if !parts.iter().any(|p| p.contains(&s)) {
            parts.push(s);
        }
        source = e.source();
    }
    parts.join(": ")
}

/// Human friendly description of a reqwest error.
pub(crate) fn describe_reqwest_error(err: &reqwest::Error, timeout: Duration) -> String {
    if err.is_timeout() {
        return format!("timeout after {}", humantime::format_duration(timeout));
    }
    // Look for a well-known IO error kind first: it is the most useful text.
    let mut source = err.source();
    while let Some(e) = source {
        if let Some(io) = e.downcast_ref::<std::io::Error>() {
            return describe_io_error(io);
        }
        source = e.source();
    }
    let chain = error_chain(err);
    // reqwest prefixes "error sending request for url (...)"; drop the URL.
    match chain.split_once("): ") {
        Some((head, tail)) if head.starts_with("error sending request") => tail.to_owned(),
        _ => chain,
    }
}

pub(crate) fn describe_io_error(err: &std::io::Error) -> String {
    use std::io::ErrorKind::*;
    match err.kind() {
        ConnectionRefused => "connection refused".to_owned(),
        ConnectionReset => "connection reset".to_owned(),
        ConnectionAborted => "connection aborted".to_owned(),
        TimedOut => "connection timed out".to_owned(),
        UnexpectedEof => "connection closed unexpectedly".to_owned(),
        _ => err.to_string(),
    }
}

pub(crate) fn format_timeout(timeout: Duration) -> String {
    format!("timeout after {}", humantime::format_duration(timeout))
}
