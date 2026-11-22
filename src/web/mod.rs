//! Embedded web server: status page, JSON API and health endpoint.

mod page;

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::config::{AuthConfig, Config};
use crate::monitor::Status;
use crate::scheduler::{unix_now, LiveMonitor, LiveState};
use crate::store::{Store, StoreError, Uptime};

const STYLE_CSS: &str = include_str!("assets/style.css");
const APP_JS: &str = include_str!("assets/app.js");
const FAVICON_SVG: &str = include_str!("assets/favicon.svg");

/// Number of recent checks shown in the response-time sparkline.
pub const HISTORY_POINTS: usize = 60;
/// Number of incidents listed on the page.
pub const INCIDENT_LIMIT: usize = 20;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub store: Store,
    pub live: LiveState,
}

/// Build the router with all routes and middleware.
pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/", get(index))
        .route("/api/status", get(api_status))
        .route("/assets/style.css", get(style))
        .route("/assets/app.js", get(script))
        .route("/favicon.svg", get(favicon))
        .route_layer(middleware::from_fn_with_state(state.clone(), basic_auth));
    Router::new()
        .route("/healthz", get(healthz))
        .merge(protected)
        .fallback(not_found)
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

/// Serve until `shutdown` is cancelled.
pub async fn serve(
    listener: tokio::net::TcpListener,
    state: AppState,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await
}

#[derive(Debug, Serialize)]
pub struct StatusSnapshot {
    pub title: String,
    pub language: &'static str,
    pub generated_at: i64,
    /// `up`, `warn`, `down` or `pending`.
    pub overall: &'static str,
    pub monitors: Vec<MonitorView>,
    pub incidents: Vec<IncidentView>,
}

#[derive(Debug, Serialize)]
pub struct MonitorView {
    #[serde(flatten)]
    pub live: LiveMonitor,
    pub uptime: Uptime,
    pub history: Vec<HistoryPoint>,
}

#[derive(Debug, Serialize)]
pub struct HistoryPoint {
    pub ts: i64,
    pub status: Status,
    pub latency_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct IncidentView {
    pub monitor: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub duration_secs: i64,
    pub ongoing: bool,
    pub message: String,
}

/// Collect everything the page and the API show.
pub async fn snapshot(state: &AppState) -> Result<StatusSnapshot, StoreError> {
    let live: Vec<LiveMonitor> = state.live.read().unwrap_or_else(|p| p.into_inner()).clone();
    let now = unix_now();
    let names: Vec<String> = live.iter().map(|m| m.name.clone()).collect();
    let (per_monitor, incidents) = state
        .store
        .run(move |s| {
            let mut out = Vec::with_capacity(names.len());
            for name in &names {
                out.push((s.uptime(name, now)?, s.recent_checks(name, HISTORY_POINTS)?));
            }
            Ok((out, s.recent_incidents(INCIDENT_LIMIT)?))
        })
        .await?;

    let overall = if live.iter().any(|m| m.status == Some(Status::Down)) {
        "down"
    } else if live.iter().any(|m| m.status == Some(Status::Warn)) {
        "warn"
    } else if !live.is_empty() && live.iter().all(|m| m.status.is_none()) {
        "pending"
    } else {
        "up"
    };

    let monitors = live
        .into_iter()
        .zip(per_monitor)
        .map(|(live, (uptime, checks))| MonitorView {
            live,
            uptime,
            history: checks
                .into_iter()
                .map(|c| HistoryPoint {
                    ts: c.ts,
                    status: c.status,
                    latency_ms: c.latency_ms,
                })
                .collect(),
        })
        .collect();
    let incidents = incidents
        .into_iter()
        .map(|i| IncidentView {
            duration_secs: i.duration_secs(now),
            ongoing: i.ended_at.is_none(),
            monitor: i.monitor,
            started_at: i.started_at,
            ended_at: i.ended_at,
            message: i.message,
        })
        .collect();

    Ok(StatusSnapshot {
        title: state.cfg.title.clone(),
        language: crate::i18n::texts(state.cfg.language).lang_code,
        generated_at: now,
        overall,
        monitors,
        incidents,
    })
}

fn internal_error(e: StoreError) -> Response {
    tracing::error!(error = %e, "cannot build status");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal error\n").into_response()
}

async fn index(State(state): State<AppState>) -> Response {
    match snapshot(&state).await {
        Ok(snap) => {
            let html = page::render(&snap, state.cfg.language);
            ([(header::CACHE_CONTROL, "no-store")], Html(html)).into_response()
        }
        Err(e) => internal_error(e),
    }
}

async fn api_status(State(state): State<AppState>) -> Response {
    match snapshot(&state).await {
        Ok(snap) => ([(header::CACHE_CONTROL, "no-store")], Json(snap)).into_response(),
        Err(e) => internal_error(e),
    }
}

async fn healthz() -> impl IntoResponse {
    ([(header::CACHE_CONTROL, "no-store")], "ok\n")
}

async fn style() -> impl IntoResponse {
    asset("text/css; charset=utf-8", STYLE_CSS)
}

async fn script() -> impl IntoResponse {
    asset("text/javascript; charset=utf-8", APP_JS)
}

async fn favicon() -> impl IntoResponse {
    asset("image/svg+xml", FAVICON_SVG)
}

fn asset(content_type: &'static str, body: &'static str) -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        body,
    )
}

async fn not_found() -> impl IntoResponse {
    (StatusCode::NOT_FOUND, "not found\n")
}

async fn security_headers(req: Request, next: Next) -> Response {
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; style-src 'self'; script-src 'self'; img-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        ),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    resp
}

async fn basic_auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(auth) = &state.cfg.web.auth else {
        return next.run(req).await;
    };
    if authorized(req.headers(), auth) {
        return next.run(req).await;
    }
    (
        StatusCode::UNAUTHORIZED,
        [(
            header::WWW_AUTHENTICATE,
            "Basic realm=\"watchpost\", charset=\"UTF-8\"",
        )],
        "authentication required\n",
    )
        .into_response()
}

fn authorized(headers: &HeaderMap, auth: &AuthConfig) -> bool {
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let Some(encoded) = value
        .strip_prefix("Basic ")
        .or_else(|| value.strip_prefix("basic "))
    else {
        return false;
    };
    let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded.trim()) else {
        return false;
    };
    let Some(sep) = decoded.iter().position(|&b| b == b':') else {
        return false;
    };
    let (user, pass) = (&decoded[..sep], &decoded[sep + 1..]);
    // Evaluate both comparisons to avoid leaking which part was wrong.
    let user_ok = constant_time_eq(user, auth.username.as_bytes());
    let pass_ok = constant_time_eq(pass, auth.password.as_bytes());
    user_ok & pass_ok
}

/// Compare without early exit on the first differing byte.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = (a.len() ^ b.len()) as u64;
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= u64::from(x ^ y);
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth() -> AuthConfig {
        AuthConfig {
            username: "admin".into(),
            password: "pa:ss".into(),
        }
    }

    fn headers(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        h
    }

    fn basic(user_pass: &str) -> String {
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(user_pass)
        )
    }

    #[test]
    fn accepts_correct_credentials() {
        assert!(authorized(&headers(&basic("admin:pa:ss")), &auth()));
    }

    #[test]
    fn rejects_wrong_or_missing_credentials() {
        assert!(!authorized(&HeaderMap::new(), &auth()));
        assert!(!authorized(&headers(&basic("admin:wrong")), &auth()));
        assert!(!authorized(&headers(&basic("admin:pa:s")), &auth()));
        assert!(!authorized(&headers(&basic("root:pa:ss")), &auth()));
        assert!(!authorized(&headers(&basic("adminpass")), &auth()));
        assert!(!authorized(&headers("Bearer abc"), &auth()));
        assert!(!authorized(&headers("Basic !!!"), &auth()));
    }

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"", b""));
    }
}
