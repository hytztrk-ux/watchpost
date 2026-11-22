//! Runs every monitor on its own interval, records results, tracks state
//! and hands events to the notification dispatcher.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde::Serialize;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::alert::{EventKind, MonitorState};
use crate::config::{Config, Monitor};
use crate::monitor::{CheckResult, Probe, ProbeError, Status};
use crate::notify::{Dispatcher, Notification};
use crate::store::Store;

/// Live, in-memory view of one monitor, shown on the status page.
#[derive(Debug, Clone, Serialize)]
pub struct LiveMonitor {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub target: String,
    pub interval_secs: u64,
    /// Confirmed status (after retries); `None` while pending.
    pub status: Option<Status>,
    /// Since when the confirmed status holds (unix seconds).
    pub since: Option<i64>,
    pub last_check: Option<i64>,
    pub last_result: Option<Status>,
    pub latency_ms: Option<i64>,
    pub message: Option<String>,
    pub cert_days_left: Option<i64>,
}

pub type LiveState = Arc<RwLock<Vec<LiveMonitor>>>;

pub fn new_live_state(cfg: &Config) -> LiveState {
    Arc::new(RwLock::new(
        cfg.monitors
            .iter()
            .map(|m| LiveMonitor {
                name: m.name.clone(),
                kind: m.kind.type_name(),
                target: m.kind.target(),
                interval_secs: m.interval.as_secs(),
                status: None,
                since: None,
                last_check: None,
                last_result: None,
                latency_ms: None,
                message: None,
                cert_days_left: None,
            })
            .collect(),
    ))
}

pub fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn latency_ms(r: &CheckResult) -> Option<i64> {
    r.latency.map(|d| d.as_millis() as i64)
}

/// Run every monitor once, concurrently. Results are in config order.
pub async fn check_all(cfg: &Config) -> Result<Vec<CheckResult>, ProbeError> {
    let probes = cfg
        .monitors
        .iter()
        .map(Probe::new)
        .collect::<Result<Vec<_>, _>>()?;
    let futures = probes.iter().map(|p| p.check());
    Ok(futures_util::future::join_all(futures).await)
}

pub struct Scheduler {
    cfg: Arc<Config>,
    store: Store,
    dispatcher: Arc<Dispatcher>,
    live: LiveState,
}

impl Scheduler {
    pub fn new(
        cfg: Arc<Config>,
        store: Store,
        dispatcher: Arc<Dispatcher>,
        live: LiveState,
    ) -> Self {
        Self {
            cfg,
            store,
            dispatcher,
            live,
        }
    }

    /// Run until `shutdown` is cancelled.
    pub async fn run(self, shutdown: CancellationToken) -> Result<(), ProbeError> {
        let probes = self
            .cfg
            .monitors
            .iter()
            .map(Probe::new)
            .collect::<Result<Vec<_>, _>>()?;

        let known: Vec<String> = self.cfg.monitors.iter().map(|m| m.name.clone()).collect();
        let now = unix_now();
        match self
            .store
            .run(move |s| s.close_orphaned_incidents(&known, now))
            .await
        {
            Ok(n) if n > 0 => tracing::info!(count = n, "closed incidents of removed monitors"),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "cannot close orphaned incidents"),
        }

        let mut tasks = JoinSet::new();
        let count = probes.len();
        for (idx, (monitor, probe)) in self.cfg.monitors.iter().cloned().zip(probes).enumerate() {
            // Spread the first checks a little so they do not all fire at once.
            let offset = Duration::from_millis((idx as u64 * 200) % 5_000).min(monitor.interval);
            let ctx = MonitorTask {
                idx,
                monitor,
                probe,
                store: self.store.clone(),
                dispatcher: self.dispatcher.clone(),
                live: self.live.clone(),
            };
            tasks.spawn(ctx.run(offset, shutdown.clone()));
        }
        tasks.spawn(prune_loop(
            self.store.clone(),
            self.cfg.storage.retention_days,
            shutdown.clone(),
        ));
        tracing::info!(monitors = count, "scheduler started");

        while let Some(res) = tasks.join_next().await {
            if let Err(e) = res {
                tracing::error!(error = %e, "monitor task failed");
            }
        }
        tracing::info!("scheduler stopped");
        Ok(())
    }
}

async fn prune_loop(store: Store, retention_days: u32, shutdown: CancellationToken) {
    let mut ticker = tokio::time::interval(Duration::from_secs(3600));
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = ticker.tick() => {}
        }
        let now = unix_now();
        match store.run(move |s| s.prune(now, retention_days)).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(deleted = n, "pruned old check results"),
            Err(e) => tracing::warn!(error = %e, "pruning failed"),
        }
    }
}

struct MonitorTask {
    idx: usize,
    monitor: Monitor,
    probe: Probe,
    store: Store,
    dispatcher: Arc<Dispatcher>,
    live: LiveState,
}

impl MonitorTask {
    async fn run(self, offset: Duration, shutdown: CancellationToken) {
        let name = self.monitor.name.clone();
        let mut state = MonitorState::new(self.monitor.retries, self.monitor.repeat_reminder);

        // Restore what we knew before a restart.
        let n = name.clone();
        let restored = self
            .store
            .run(move |s| Ok((s.open_incident_for(&n)?, s.last_check(&n)?)))
            .await;
        match restored {
            Ok((incident, last)) => {
                if let Some(inc) = incident {
                    state.restore_down(inc.started_at, unix_now());
                    tracing::info!(monitor = %name, "monitor was down before restart");
                }
                self.update_live(|l| {
                    if let Some(last) = last {
                        l.last_check = Some(last.ts);
                        l.last_result = Some(last.status);
                        l.latency_ms = last.latency_ms;
                        l.message = Some(last.message);
                    }
                    l.status = state.confirmed();
                    l.since = state.since();
                });
            }
            Err(e) => tracing::warn!(monitor = %name, error = %e, "cannot restore state"),
        }

        let start = tokio::time::Instant::now() + offset;
        let mut ticker = tokio::time::interval_at(start, self.monitor.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = ticker.tick() => {}
            }
            let result = tokio::select! {
                _ = shutdown.cancelled() => break,
                r = self.probe.check() => r,
            };
            self.handle(&mut state, result).await;
        }
        tracing::debug!(monitor = %name, "monitor stopped");
    }

    async fn handle(&self, state: &mut MonitorState, result: CheckResult) {
        let m = &self.monitor;
        let now = unix_now();
        let latency = latency_ms(&result);
        tracing::debug!(
            monitor = %m.name,
            status = %result.status,
            latency_ms = latency,
            detail = %result.message,
            "check finished"
        );

        let (n, msg, status) = (m.name.clone(), result.message.clone(), result.status);
        if let Err(e) = self
            .store
            .run(move |s| s.record_check(&n, now, status, latency, &msg))
            .await
        {
            tracing::warn!(monitor = %m.name, error = %e, "cannot store check result");
        }

        let previous_since = state.since();
        let events = state.observe(result.status, now);
        let confirmed = state.confirmed().unwrap_or(result.status);

        for kind in events {
            let down_since = match kind {
                EventKind::Down => Some(now),
                EventKind::Recovered => previous_since,
                EventKind::Reminder if confirmed == Status::Down => state.since(),
                _ => None,
            };
            let (n, msg) = (m.name.clone(), result.message.clone());
            let stored = match kind {
                EventKind::Down => {
                    self.store
                        .run(move |s| s.open_incident(&n, now, &msg).map(|_| ()))
                        .await
                }
                EventKind::Recovered => {
                    self.store
                        .run(move |s| s.close_incidents(&n, now).map(|_| ()))
                        .await
                }
                _ => Ok(()),
            };
            if let Err(e) = stored {
                tracing::warn!(monitor = %m.name, error = %e, "cannot store incident");
            }

            match kind {
                EventKind::Down | EventKind::Reminder => tracing::warn!(
                    monitor = %m.name,
                    event = kind.as_str(),
                    detail = %result.message,
                    "state change"
                ),
                _ => tracing::info!(
                    monitor = %m.name,
                    event = kind.as_str(),
                    detail = %result.message,
                    "state change"
                ),
            }

            if !m.notify.is_empty() {
                self.dispatcher.dispatch(
                    Notification {
                        kind,
                        status: confirmed,
                        monitor: m.name.clone(),
                        monitor_type: m.kind.type_name().to_owned(),
                        target: m.kind.target(),
                        message: result.message.clone(),
                        at: now,
                        down_since,
                    },
                    &m.notify,
                );
            }
        }

        self.update_live(|l| {
            l.status = state.confirmed();
            l.since = state.since();
            l.last_check = Some(now);
            l.last_result = Some(result.status);
            l.latency_ms = latency;
            l.message = Some(result.message.clone());
            if result.cert_days_left.is_some() {
                l.cert_days_left = result.cert_days_left;
            }
        });
    }

    fn update_live(&self, f: impl FnOnce(&mut LiveMonitor)) {
        let mut guard = self.live.write().unwrap_or_else(|p| p.into_inner());
        if let Some(l) = guard.get_mut(self.idx) {
            f(l);
        }
    }
}
