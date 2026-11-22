//! Notification channels and delivery.
//!
//! Delivery happens in background tasks so a slow channel never delays
//! checks. Failed deliveries are retried a few times, then logged.

mod channels;

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tokio_util::task::TaskTracker;

use crate::alert::EventKind;
use crate::config::{Config, Language, NotifierConfig};
use crate::i18n::{self, texts};
use crate::monitor::Status;

pub use channels::Channel;

/// An event to deliver.
#[derive(Debug, Clone, Serialize)]
pub struct Notification {
    pub kind: EventKind,
    /// Confirmed status of the monitor after the event.
    pub status: Status,
    pub monitor: String,
    pub monitor_type: String,
    pub target: String,
    /// Detail of the check that triggered the event.
    pub message: String,
    /// Unix seconds.
    pub at: i64,
    /// Start of the outage (for `down`, `reminder` and `recovered`).
    pub down_since: Option<i64>,
}

impl Notification {
    /// The message sent by `watchpost test-notify`.
    pub fn test(now: i64) -> Self {
        Self {
            kind: EventKind::Test,
            status: Status::Up,
            monitor: "watchpost".into(),
            monitor_type: "test".into(),
            target: "-".into(),
            message: String::new(),
            at: now,
            down_since: None,
        }
    }
}

/// Settings shared by every message.
#[derive(Debug, Clone)]
pub struct MessageContext {
    pub title: String,
    pub language: Language,
    pub public_url: Option<String>,
}

impl MessageContext {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            title: cfg.title.clone(),
            language: cfg.language,
            public_url: cfg.web.public_url.clone(),
        }
    }
}

/// Human readable text of a notification.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    pub subject: String,
    pub text: String,
}

pub fn render(n: &Notification, ctx: &MessageContext) -> Rendered {
    let t = texts(ctx.language);
    let (icon, label) = match n.kind {
        EventKind::Down => ("🔴", t.n_down),
        EventKind::Recovered => ("🟢", t.n_recovered),
        EventKind::CertExpiring => ("🟠", t.n_cert),
        EventKind::Reminder if n.status == Status::Warn => ("🟠", t.n_still_cert),
        EventKind::Reminder => ("🔴", t.n_still_down),
        EventKind::Test => ("🔔", t.n_test),
    };
    let subject = if n.kind == EventKind::Test {
        format!("[{}] {label}", ctx.title)
    } else {
        format!("[{}] {label}: {}", ctx.title, n.monitor)
    };

    let mut lines = vec![format!("{icon} {subject}")];
    if n.kind == EventKind::Test {
        lines.push(String::new());
        lines.push(t.n_test_body.to_owned());
    } else {
        lines.push(String::new());
        lines.push(format!("{}: {} ({})", t.n_target, n.target, n.monitor_type));
        if !n.message.is_empty() {
            lines.push(format!("{}: {}", t.n_detail, n.message));
        }
        if let Some(since) = n.down_since {
            if matches!(n.kind, EventKind::Recovered | EventKind::Reminder) {
                lines.push(format!(
                    "{}: {}",
                    t.n_down_for,
                    i18n::format_duration(n.at - since, ctx.language)
                ));
            }
        }
    }
    lines.push(format!("{}: {}", t.n_time, format_time(n.at)));
    if let Some(url) = &ctx.public_url {
        lines.push(format!("{}: {url}", t.n_status_page));
    }
    Rendered {
        subject,
        text: lines.join("\n"),
    }
}

pub(crate) fn format_time(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| d.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| ts.to_string())
}

pub(crate) fn format_rfc3339(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|| ts.to_string())
}

#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    #[error("request failed: {0}")]
    Http(String),
    #[error("server answered HTTP {status}: {body}")]
    Status { status: u16, body: String },
    #[error("email: {0}")]
    Email(String),
    #[error("unknown notifier `{0}`")]
    Unknown(String),
    #[error("cannot build HTTP client: {0}")]
    Client(String),
}

impl From<reqwest::Error> for NotifyError {
    fn from(e: reqwest::Error) -> Self {
        // Never log URLs: they contain bot tokens and webhook secrets.
        NotifyError::Http(crate::monitor::error_chain(&e.without_url()))
    }
}

/// Owns all channels and delivers notifications.
pub struct Dispatcher {
    channels: Vec<(String, Channel)>,
    ctx: MessageContext,
    tracker: TaskTracker,
    retry_delays: Vec<Duration>,
}

impl Dispatcher {
    pub fn new(cfg: &Config) -> Result<Arc<Dispatcher>, NotifyError> {
        Self::with_notifiers(
            &cfg.notifiers,
            MessageContext::from_config(cfg),
            vec![Duration::from_secs(2), Duration::from_secs(10)],
        )
    }

    /// Build a dispatcher. `retry_delays` are the pauses between delivery
    /// attempts; its length is the number of retries.
    pub fn with_notifiers(
        notifiers: &[NotifierConfig],
        ctx: MessageContext,
        retry_delays: Vec<Duration>,
    ) -> Result<Arc<Dispatcher>, NotifyError> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("watchpost/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(1)
            .build()
            .map_err(|e| NotifyError::Client(e.to_string()))?;
        let channels = notifiers
            .iter()
            .map(|n| Ok((n.name.clone(), Channel::new(&n.kind, client.clone())?)))
            .collect::<Result<Vec<_>, NotifyError>>()?;
        Ok(Arc::new(Dispatcher {
            channels,
            ctx,
            tracker: TaskTracker::new(),
            retry_delays,
        }))
    }

    pub fn names(&self) -> Vec<String> {
        self.channels.iter().map(|(n, _)| n.clone()).collect()
    }

    pub fn context(&self) -> &MessageContext {
        &self.ctx
    }

    /// Deliver once to one channel and wait for the result.
    pub async fn send_to(&self, name: &str, n: &Notification) -> Result<(), NotifyError> {
        let (_, channel) = self
            .channels
            .iter()
            .find(|(cname, _)| cname == name)
            .ok_or_else(|| NotifyError::Unknown(name.to_owned()))?;
        channel.send(n, &self.ctx).await
    }

    /// Deliver in the background to the named channels, with retries.
    pub fn dispatch(self: &Arc<Self>, n: Notification, targets: &[String]) {
        for name in targets {
            let this = Arc::clone(self);
            let name = name.clone();
            let n = n.clone();
            self.tracker.spawn(async move {
                let mut attempt = 0;
                loop {
                    match this.send_to(&name, &n).await {
                        Ok(()) => {
                            tracing::info!(
                                notifier = %name,
                                monitor = %n.monitor,
                                event = n.kind.as_str(),
                                "notification sent"
                            );
                            return;
                        }
                        Err(e) => {
                            let Some(delay) = this.retry_delays.get(attempt).copied() else {
                                tracing::error!(
                                    notifier = %name,
                                    monitor = %n.monitor,
                                    event = n.kind.as_str(),
                                    error = %e,
                                    "notification failed, giving up"
                                );
                                return;
                            };
                            tracing::warn!(
                                notifier = %name,
                                monitor = %n.monitor,
                                error = %e,
                                retry_in = ?delay,
                                "notification failed, retrying"
                            );
                            attempt += 1;
                            tokio::time::sleep(delay).await;
                        }
                    }
                }
            });
        }
    }

    /// Wait (at most `timeout`) for deliveries that are still in flight.
    pub async fn shutdown(&self, timeout: Duration) {
        self.tracker.close();
        if tokio::time::timeout(timeout, self.tracker.wait())
            .await
            .is_err()
        {
            tracing::warn!(
                pending = self.tracker.len(),
                "gave up waiting for pending notifications"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(lang: Language) -> MessageContext {
        MessageContext {
            title: "Prod".into(),
            language: lang,
            public_url: Some("https://status.example.com".into()),
        }
    }

    fn sample(kind: EventKind, status: Status) -> Notification {
        Notification {
            kind,
            status,
            monitor: "API".into(),
            monitor_type: "http".into(),
            target: "https://api.example.com".into(),
            message: "connection refused".into(),
            at: 1_700_000_000,
            down_since: Some(1_700_000_000 - 3725),
        }
    }

    #[test]
    fn renders_down_in_english() {
        let r = render(&sample(EventKind::Down, Status::Down), &ctx(Language::En));
        assert_eq!(r.subject, "[Prod] DOWN: API");
        assert!(r.text.contains("Target: https://api.example.com (http)"));
        assert!(r.text.contains("Detail: connection refused"));
        assert!(r.text.contains("2023-11-14 22:13:20 UTC"), "{}", r.text);
        assert!(r.text.contains("Status page: https://status.example.com"));
        assert!(
            !r.text.contains("Down for"),
            "no duration on the down event"
        );
    }

    #[test]
    fn renders_recovery_with_duration() {
        let r = render(
            &sample(EventKind::Recovered, Status::Up),
            &ctx(Language::En),
        );
        assert_eq!(r.subject, "[Prod] RECOVERED: API");
        assert!(r.text.contains("Down for: 1h 2m"), "{}", r.text);
    }

    #[test]
    fn renders_turkish() {
        let r = render(&sample(EventKind::Down, Status::Down), &ctx(Language::Tr));
        assert_eq!(r.subject, "[Prod] ERİŞİLEMİYOR: API");
        assert!(r.text.contains("Hedef:"));
        let r = render(
            &sample(EventKind::Recovered, Status::Up),
            &ctx(Language::Tr),
        );
        assert!(r.text.contains("Kesinti süresi: 1sa 2dk"), "{}", r.text);
    }

    #[test]
    fn reminder_text_depends_on_status() {
        let down = render(
            &sample(EventKind::Reminder, Status::Down),
            &ctx(Language::En),
        );
        assert!(down.subject.contains("STILL DOWN"));
        let warn = render(
            &sample(EventKind::Reminder, Status::Warn),
            &ctx(Language::En),
        );
        assert!(warn.subject.contains("CERTIFICATE STILL EXPIRING"));
    }

    #[test]
    fn renders_test_message() {
        let r = render(&Notification::test(0), &ctx(Language::En));
        assert_eq!(r.subject, "[Prod] TEST");
        assert!(r.text.contains("test notification"));
    }
}
