//! Scheduler + store + notifications + web, wired together like `watchpost run`.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{closed_port, config, Recorder, Target};
use tokio_util::sync::CancellationToken;
use watchpost::monitor::Status;
use watchpost::notify::{Dispatcher, MessageContext};
use watchpost::scheduler::{new_live_state, LiveState, Scheduler};
use watchpost::store::Store;
use watchpost::web::{self, AppState};

struct Running {
    store: Store,
    live: LiveState,
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<()>,
    web: std::net::SocketAddr,
}

async fn start(text: &str) -> Running {
    let cfg = Arc::new(config(text));
    let store = Store::open_in_memory().unwrap();
    let dispatcher = Dispatcher::with_notifiers(
        &cfg.notifiers,
        MessageContext::from_config(&cfg),
        vec![Duration::from_millis(50)],
    )
    .unwrap();
    let live = new_live_state(&cfg);
    let shutdown = CancellationToken::new();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let web_addr = listener.local_addr().unwrap();
    let state = AppState {
        cfg: cfg.clone(),
        store: store.clone(),
        live: live.clone(),
    };
    let token = shutdown.clone();
    tokio::spawn(async move { web::serve(listener, state, token).await.unwrap() });

    let scheduler = Scheduler::new(cfg, store.clone(), dispatcher.clone(), live.clone());
    let token = shutdown.clone();
    let task = tokio::spawn(async move {
        scheduler.run(token).await.unwrap();
        dispatcher.shutdown(Duration::from_secs(2)).await;
    });
    Running {
        store,
        live,
        shutdown,
        task,
        web: web_addr,
    }
}

impl Running {
    async fn stop(self) {
        self.shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(5), self.task)
            .await
            .expect("scheduler stops promptly")
            .unwrap();
    }

    fn status_of(&self, idx: usize) -> Option<Status> {
        self.live.read().unwrap()[idx].status
    }

    async fn wait_status(&self, idx: usize, want: Status, timeout: Duration) {
        let deadline = tokio::time::Instant::now() + timeout;
        while self.status_of(idx) != Some(want) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "monitor {idx} did not become {want} (is {:?})",
                self.status_of(idx)
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

#[tokio::test]
async fn detects_down_and_recovery_and_notifies_once_each() {
    let target = Target::start().await;
    let hook = Recorder::start().await;
    let text = format!(
        r#"
title = "Test"
[[notifier]]
name = "hook"
type = "webhook"
url = "{hook}"

[[monitor]]
name = "flaky"
type = "http"
url = "{url}"
interval = "1s"
timeout = "1s"
retries = 1
"#,
        hook = hook.url("/hook"),
        url = target.url("/toggle")
    );
    let run = start(&text).await;

    run.wait_status(0, Status::Up, Duration::from_secs(5)).await;
    assert!(hook.requests().is_empty(), "startup is silent");

    target.set_healthy(false);
    run.wait_status(0, Status::Down, Duration::from_secs(6))
        .await;
    let reqs = hook.wait_for(1, Duration::from_secs(3)).await;
    let body = reqs[0].json();
    assert_eq!(body["event"], "down");
    assert_eq!(body["monitor"], "flaky");
    assert!(body["message"].as_str().unwrap().contains("HTTP 500"));

    // Staying down must not send more notifications.
    tokio::time::sleep(Duration::from_millis(2200)).await;
    assert_eq!(hook.requests().len(), 1, "no alert storm");

    let open = run.store.open_incident_for("flaky").unwrap();
    assert!(open.is_some(), "incident opened");

    target.set_healthy(true);
    run.wait_status(0, Status::Up, Duration::from_secs(5)).await;
    let reqs = hook.wait_for(2, Duration::from_secs(3)).await;
    let body = reqs[1].json();
    assert_eq!(body["event"], "recovered");
    assert!(body["duration_seconds"].as_i64().unwrap() >= 2);
    assert!(run.store.open_incident_for("flaky").unwrap().is_none());

    let api: serde_json::Value = reqwest::get(format!("http://{}/api/status", run.web))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(api["overall"], "up");
    assert_eq!(api["incidents"].as_array().unwrap().len(), 1);
    let uptime = api["monitors"][0]["uptime"]["h24"].as_f64().unwrap();
    assert!(uptime > 0.0 && uptime < 1.0, "{uptime}");

    run.stop().await;
}

#[tokio::test]
async fn up_and_down_monitors_side_by_side() {
    let target = Target::start().await;
    let hook = Recorder::start().await;
    let port = closed_port().await;
    let text = format!(
        r#"
[[notifier]]
name = "hook"
type = "webhook"
url = "{hook}"

[[monitor]]
name = "web"
type = "http"
url = "{url}"
interval = "1s"

[[monitor]]
name = "db"
type = "tcp"
host = "127.0.0.1"
port = {port}
interval = "1s"
retries = 0

[[monitor]]
name = "silent"
type = "tcp"
host = "127.0.0.1"
port = {port}
interval = "1s"
retries = 0
notify = []
"#,
        hook = hook.url("/"),
        url = target.url("/ok"),
    );
    let run = start(&text).await;
    run.wait_status(0, Status::Up, Duration::from_secs(5)).await;
    run.wait_status(1, Status::Down, Duration::from_secs(10))
        .await;
    run.wait_status(2, Status::Down, Duration::from_secs(10))
        .await;

    let reqs = hook.wait_for(1, Duration::from_secs(3)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let reqs_after = hook.requests();
    assert_eq!(reqs_after.len(), 1, "only `db` notifies: {reqs_after:?}");
    assert_eq!(reqs[0].json()["monitor"], "db");

    let resp = reqwest::get(format!("http://{}/", run.web)).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/html"));
    let html = resp.text().await.unwrap();
    assert!(html.contains("Some systems are down"));
    assert!(html.contains(">web</h3>"));
    assert!(html.contains(">db</h3>"));
    assert!(html.contains("connection refused"));

    run.stop().await;
}

#[tokio::test]
async fn restart_with_open_incident_does_not_renotify() {
    let hook = Recorder::start().await;
    let port = closed_port().await;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("n.db");
    let text = format!(
        r#"
[storage]
path = '{db}'
[[notifier]]
name = "hook"
type = "webhook"
url = "{hook}"
[[monitor]]
name = "db"
type = "tcp"
host = "127.0.0.1"
port = {port}
interval = "1s"
retries = 0
"#,
        db = db.display(),
        hook = hook.url("/"),
    );
    let cfg = Arc::new(config(&text));

    for round in 0..2 {
        let store = Store::open(&cfg.storage.path).unwrap();
        let dispatcher = Dispatcher::new(&cfg).unwrap();
        let live = new_live_state(&cfg);
        let token = CancellationToken::new();
        let s = Scheduler::new(cfg.clone(), store.clone(), dispatcher.clone(), live.clone());
        let t = token.clone();
        let task = tokio::spawn(async move { s.run(t).await.unwrap() });
        // Wait for at least one real check in this round (connection
        // refusal can take a couple of seconds on some platforms).
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while store.recent_checks("db", 100).unwrap().len() <= round {
            assert!(
                tokio::time::Instant::now() < deadline,
                "no check in round {round}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        token.cancel();
        task.await.unwrap();
        dispatcher.shutdown(Duration::from_secs(2)).await;
        assert_eq!(
            live.read().unwrap()[0].status,
            Some(Status::Down),
            "round {round}"
        );
    }
    assert_eq!(
        hook.requests().len(),
        1,
        "down is notified once across restarts"
    );
}
