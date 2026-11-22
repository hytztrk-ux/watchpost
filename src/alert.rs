//! Per-monitor state machine deciding when to notify.
//!
//! Rules:
//!
//! * A monitor becomes **down** only after `retries + 1` consecutive failed
//!   checks. Shorter failure streaks are ignored.
//! * Notifications are sent on **transitions** only: down, recovered and
//!   certificate expiring. A monitor that stays down does not notify again,
//!   unless `repeat_reminder` is set, in which case a reminder goes out at
//!   most once per interval.
//! * The first successful check after start-up is silent (nothing changed
//!   from the user's point of view); a monitor that is down from the start
//!   does notify.

use std::time::Duration;

use serde::Serialize;

use crate::monitor::Status;

/// Something worth telling a human about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Down,
    Recovered,
    CertExpiring,
    /// Still down (or still expiring) after `repeat_reminder`.
    Reminder,
    /// Sent by `watchpost test-notify`.
    Test,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Down => "down",
            EventKind::Recovered => "recovered",
            EventKind::CertExpiring => "cert_expiring",
            EventKind::Reminder => "reminder",
            EventKind::Test => "test",
        }
    }
}

#[derive(Debug, Clone)]
pub struct MonitorState {
    retries: u32,
    repeat: Option<i64>,
    confirmed: Option<Status>,
    failures: u32,
    /// When the current confirmed state started (unix seconds).
    since: Option<i64>,
    /// When the current bad state (down / warn) was last notified.
    last_notified: Option<i64>,
}

impl MonitorState {
    pub fn new(retries: u32, repeat_reminder: Option<Duration>) -> Self {
        Self {
            retries,
            repeat: repeat_reminder
                .map(|d| d.as_secs() as i64)
                .filter(|s| *s > 0),
            confirmed: None,
            failures: 0,
            since: None,
            last_notified: None,
        }
    }

    /// Restore a down state persisted before a restart (an open incident),
    /// so that a restart does not send a second "down" notification.
    pub fn restore_down(&mut self, since: i64, now: i64) {
        self.confirmed = Some(Status::Down);
        self.failures = self.retries + 1;
        self.since = Some(since);
        self.last_notified = Some(now);
    }

    /// Confirmed state; `None` while still pending (no decision yet).
    pub fn confirmed(&self) -> Option<Status> {
        self.confirmed
    }

    /// Start of the current confirmed state.
    pub fn since(&self) -> Option<i64> {
        self.since
    }

    /// Consecutive failed checks so far.
    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// Feed one check result; returns the events to notify about.
    pub fn observe(&mut self, status: Status, now: i64) -> Vec<EventKind> {
        let mut events = Vec::new();
        match status {
            Status::Down => {
                self.failures = self.failures.saturating_add(1);
                if self.confirmed == Some(Status::Down) {
                    if self.reminder_due(now) {
                        events.push(EventKind::Reminder);
                        self.last_notified = Some(now);
                    }
                } else if self.failures > self.retries {
                    self.confirmed = Some(Status::Down);
                    self.since = Some(now);
                    self.last_notified = Some(now);
                    events.push(EventKind::Down);
                }
            }
            Status::Up | Status::Warn => {
                self.failures = 0;
                let previous = self.confirmed.replace(status);
                match (previous, status) {
                    (Some(Status::Down), _) => {
                        events.push(EventKind::Recovered);
                        self.since = Some(now);
                        self.last_notified = None;
                        if status == Status::Warn {
                            events.push(EventKind::CertExpiring);
                            self.last_notified = Some(now);
                        }
                    }
                    (Some(Status::Warn), Status::Warn) => {
                        if self.reminder_due(now) {
                            events.push(EventKind::Reminder);
                            self.last_notified = Some(now);
                        }
                    }
                    (_, Status::Warn) => {
                        events.push(EventKind::CertExpiring);
                        self.since = Some(now);
                        self.last_notified = Some(now);
                    }
                    (Some(Status::Up), Status::Up) => {}
                    (_, _) => {
                        // Pending -> up, or warn -> up (certificate renewed).
                        self.since = Some(now);
                        self.last_notified = None;
                    }
                }
            }
        }
        events
    }

    fn reminder_due(&self, now: i64) -> bool {
        match (self.repeat, self.last_notified) {
            (Some(every), Some(last)) => now - last >= every,
            (Some(_), None) => true,
            (None, _) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use EventKind::*;
    use Status::{Down as D, Up as U, Warn as W};

    fn feed(state: &mut MonitorState, seq: &[(Status, i64)]) -> Vec<(i64, EventKind)> {
        let mut out = Vec::new();
        for &(s, t) in seq {
            for e in state.observe(s, t) {
                out.push((t, e));
            }
        }
        out
    }

    #[test]
    fn first_success_is_silent() {
        let mut s = MonitorState::new(0, None);
        assert!(s.observe(U, 0).is_empty());
        assert_eq!(s.confirmed(), Some(U));
        assert!(s.observe(U, 60).is_empty());
    }

    #[test]
    fn down_from_start_notifies() {
        let mut s = MonitorState::new(0, None);
        assert_eq!(s.observe(D, 0), vec![Down]);
        assert_eq!(s.confirmed(), Some(D));
        assert_eq!(s.since(), Some(0));
    }

    #[test]
    fn retries_delay_down() {
        let mut s = MonitorState::new(2, None);
        assert!(s.observe(U, 0).is_empty());
        assert!(s.observe(D, 10).is_empty());
        assert!(s.observe(D, 20).is_empty());
        assert_eq!(s.confirmed(), Some(U), "still up while retrying");
        assert_eq!(s.failures(), 2);
        assert_eq!(s.observe(D, 30), vec![Down]);
        assert_eq!(s.since(), Some(30));
    }

    #[test]
    fn a_success_resets_the_failure_streak() {
        let mut s = MonitorState::new(1, None);
        let ev = feed(&mut s, &[(U, 0), (D, 1), (U, 2), (D, 3), (U, 4), (D, 5)]);
        assert!(ev.is_empty(), "{ev:?}");
        assert_eq!(s.confirmed(), Some(U));
    }

    #[test]
    fn no_alert_storm_while_down() {
        let mut s = MonitorState::new(0, None);
        let seq: Vec<_> = (0..100).map(|i| (D, i * 60)).collect();
        assert_eq!(feed(&mut s, &seq), vec![(0, Down)]);
    }

    #[test]
    fn recovery_is_notified_once() {
        let mut s = MonitorState::new(0, None);
        let ev = feed(&mut s, &[(D, 0), (D, 60), (U, 120), (U, 180)]);
        assert_eq!(ev, vec![(0, Down), (120, Recovered)]);
        assert_eq!(s.since(), Some(120));
    }

    #[test]
    fn flapping_produces_one_event_per_transition() {
        let mut s = MonitorState::new(0, None);
        let ev = feed(&mut s, &[(U, 0), (D, 1), (U, 2), (D, 3), (U, 4)]);
        assert_eq!(
            ev,
            vec![(1, Down), (2, Recovered), (3, Down), (4, Recovered)]
        );
    }

    #[test]
    fn repeat_reminder_while_down() {
        let mut s = MonitorState::new(0, Some(Duration::from_secs(3600)));
        let seq: Vec<_> = (0..=150).map(|m| (D, m * 60)).collect();
        let ev = feed(&mut s, &seq);
        assert_eq!(ev, vec![(0, Down), (3600, Reminder), (7200, Reminder)]);
    }

    #[test]
    fn recovery_resets_reminder_clock() {
        let mut s = MonitorState::new(0, Some(Duration::from_secs(100)));
        let ev = feed(&mut s, &[(D, 0), (U, 50), (D, 60), (D, 150), (D, 160)]);
        assert_eq!(
            ev,
            vec![(0, Down), (50, Recovered), (60, Down), (160, Reminder)]
        );
    }

    #[test]
    fn cert_expiring_is_notified_once() {
        let mut s = MonitorState::new(0, None);
        let ev = feed(&mut s, &[(U, 0), (W, 10), (W, 20), (W, 30)]);
        assert_eq!(ev, vec![(10, CertExpiring)]);
        assert_eq!(s.confirmed(), Some(W));
    }

    #[test]
    fn cert_expiring_from_start_notifies() {
        let mut s = MonitorState::new(0, None);
        assert_eq!(s.observe(W, 0), vec![CertExpiring]);
    }

    #[test]
    fn cert_expiring_reminders() {
        let mut s = MonitorState::new(0, Some(Duration::from_secs(86_400)));
        let ev = feed(&mut s, &[(W, 0), (W, 43_200), (W, 86_400), (W, 90_000)]);
        assert_eq!(ev, vec![(0, CertExpiring), (86_400, Reminder)]);
    }

    #[test]
    fn renewed_certificate_is_silent() {
        let mut s = MonitorState::new(0, None);
        let ev = feed(&mut s, &[(W, 0), (U, 10), (U, 20)]);
        assert_eq!(ev, vec![(0, CertExpiring)]);
        assert_eq!(s.confirmed(), Some(U));
    }

    #[test]
    fn recovery_into_warning_sends_both() {
        let mut s = MonitorState::new(0, None);
        let ev = feed(&mut s, &[(D, 0), (W, 10)]);
        assert_eq!(ev, vec![(0, Down), (10, Recovered), (10, CertExpiring)]);
    }

    #[test]
    fn warn_to_down_respects_retries() {
        let mut s = MonitorState::new(1, None);
        let ev = feed(&mut s, &[(W, 0), (D, 10), (D, 20)]);
        assert_eq!(ev, vec![(0, CertExpiring), (20, Down)]);
    }

    #[test]
    fn restored_down_state_does_not_renotify() {
        let mut s = MonitorState::new(0, None);
        s.restore_down(-500, 0);
        assert!(s.observe(D, 60).is_empty());
        assert_eq!(s.since(), Some(-500));
        assert_eq!(s.observe(U, 120), vec![Recovered]);
    }
}
