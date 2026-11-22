//! Shared helpers for integration tests. Everything listens on 127.0.0.1
//! with ephemeral ports; no test needs the internet.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, get};
use axum::Router;

use watchpost::config::{Config, Monitor};

/// Start an axum router on an ephemeral port.
pub async fn serve(router: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    addr
}

/// A port on which nothing is listening (bound once, then released).
pub async fn closed_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}

/// A target web service with a few useful endpoints.
///
/// * `/ok`       – 200 "hello watchpost world"
/// * `/fail`     – 503
/// * `/slow`     – 200 after 3 seconds
/// * `/redirect` – 302 to `/ok`
/// * `/echo`     – 200, body = method + value of `x-test` header
/// * `/toggle`   – 200 or 503, controlled by [`Target::set_healthy`]
pub struct Target {
    pub addr: SocketAddr,
    healthy: Arc<AtomicBool>,
}

impl Target {
    pub async fn start() -> Target {
        let healthy = Arc::new(AtomicBool::new(true));
        let router = Router::new()
            .route("/ok", get(|| async { "hello watchpost world" }))
            .route(
                "/fail",
                get(|| async { (StatusCode::SERVICE_UNAVAILABLE, "maintenance") }),
            )
            .route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    "late"
                }),
            )
            .route("/redirect", get(|| async { Redirect::to("/ok") }))
            .route(
                "/echo",
                any(|method: Method, headers: HeaderMap| async move {
                    let x = headers
                        .get("x-test")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("-")
                        .to_owned();
                    format!("{method} {x}")
                }),
            )
            .route(
                "/toggle",
                get(|State(h): State<Arc<AtomicBool>>| async move {
                    if h.load(Ordering::SeqCst) {
                        (StatusCode::OK, "fine")
                    } else {
                        (StatusCode::INTERNAL_SERVER_ERROR, "broken")
                    }
                }),
            )
            .with_state(healthy.clone());
        let addr = serve(router).await;
        Target { addr, healthy }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    pub fn set_healthy(&self, v: bool) {
        self.healthy.store(v, Ordering::SeqCst);
    }
}

/// One request received by the [`Recorder`].
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Recorded {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).expect("request body is JSON")
    }
}

type RecorderState = (Arc<Mutex<Vec<Recorded>>>, Arc<AtomicUsize>, Arc<AtomicU16>);

/// A mock HTTP endpoint that records every request it receives. It answers
/// with `fail_first` error responses (HTTP 500) before succeeding.
#[derive(Clone)]
pub struct Recorder {
    pub addr: SocketAddr,
    requests: Arc<Mutex<Vec<Recorded>>>,
    fail_first: Arc<AtomicUsize>,
    status: Arc<AtomicU16>,
}

impl Recorder {
    pub async fn start() -> Recorder {
        Self::start_with(0, 200).await
    }

    pub async fn start_with(fail_first: usize, status: u16) -> Recorder {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let fail_first = Arc::new(AtomicUsize::new(fail_first));
        let status = Arc::new(AtomicU16::new(status));
        let state = (requests.clone(), fail_first.clone(), status.clone());
        let router = Router::new().fallback(any(
            |State((reqs, fail, status)): State<RecorderState>,
             method: Method,
             uri: Uri,
             headers: HeaderMap,
             body: Bytes| async move {
                reqs.lock().unwrap().push(Recorded {
                    method: method.to_string(),
                    path: uri.path().to_owned(),
                    headers,
                    body,
                });
                let remaining = fail.load(Ordering::SeqCst);
                if remaining > 0 {
                    fail.store(remaining - 1, Ordering::SeqCst);
                    return (StatusCode::INTERNAL_SERVER_ERROR, "try again").into_response();
                }
                let code = StatusCode::from_u16(status.load(Ordering::SeqCst)).unwrap();
                let resp: Response = (code, "{\"ok\":true}").into_response();
                resp
            },
        ));
        let addr = serve(router.with_state(state)).await;
        Recorder {
            addr,
            requests,
            fail_first,
            status,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }

    /// Wait until at least `n` requests arrived (or panic after `timeout`).
    pub async fn wait_for(&self, n: usize, timeout: Duration) -> Vec<Recorded> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let reqs = self.requests();
            if reqs.len() >= n {
                return reqs;
            }
            if tokio::time::Instant::now() > deadline {
                panic!("expected {n} request(s), got {}: {reqs:?}", reqs.len());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

/// Parse a config and return it (panics with the error otherwise).
pub fn config(text: &str) -> Config {
    Config::parse(text, &|_| None).unwrap_or_else(|e| panic!("bad test config: {e}"))
}

/// Parse a config with exactly one monitor and return that monitor.
pub fn monitor(text: &str) -> Monitor {
    let mut cfg = config(text);
    assert_eq!(cfg.monitors.len(), 1);
    cfg.monitors.remove(0)
}
