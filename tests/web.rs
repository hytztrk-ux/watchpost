//! HTTP endpoints of the embedded web server.

mod common;

use std::sync::Arc;

use common::config;
use tokio_util::sync::CancellationToken;
use watchpost::monitor::Status;
use watchpost::scheduler::new_live_state;
use watchpost::store::Store;
use watchpost::web::{self, AppState};

async fn start(text: &str) -> (String, AppState) {
    let cfg = Arc::new(config(text));
    let state = AppState {
        live: new_live_state(&cfg),
        cfg,
        store: Store::open_in_memory().unwrap(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let s = state.clone();
    tokio::spawn(async move { web::serve(listener, s, CancellationToken::new()).await });
    (base, state)
}

const ONE: &str = "[[monitor]]\nname = \"site\"\ntype = \"tcp\"\nhost = \"127.0.0.1\"\nport = 9\n";

#[tokio::test]
async fn healthz_is_ok() {
    let (base, _) = start(ONE).await;
    let resp = reqwest::get(format!("{base}/healthz")).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "ok\n");
}

#[tokio::test]
async fn api_reports_pending_before_first_check() {
    let (base, _) = start(ONE).await;
    let v: serde_json::Value = reqwest::get(format!("{base}/api/status"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["overall"], "pending");
    assert_eq!(v["monitors"][0]["name"], "site");
    assert_eq!(v["monitors"][0]["type"], "tcp");
    assert_eq!(v["monitors"][0]["target"], "127.0.0.1:9");
    assert!(v["monitors"][0]["status"].is_null());
    assert!(v["monitors"][0]["uptime"]["h24"].is_null());
    assert_eq!(v["incidents"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn api_and_page_show_recorded_data() {
    let (base, state) = start(&format!(
        "language = \"tr\"\ntitle = \"Durum <Sayfası>\"\n{ONE}"
    ))
    .await;
    let now = watchpost::scheduler::unix_now();
    state
        .store
        .record_check("site", now - 60, Status::Up, Some(12), "connected")
        .unwrap();
    state
        .store
        .record_check("site", now, Status::Down, None, "connection refused")
        .unwrap();
    state
        .store
        .open_incident("site", now, "connection refused")
        .unwrap();
    {
        let mut live = state.live.write().unwrap();
        live[0].status = Some(Status::Down);
        live[0].last_check = Some(now);
        live[0].message = Some("connection refused".into());
    }

    let v: serde_json::Value = reqwest::get(format!("{base}/api/status"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["overall"], "down");
    assert_eq!(v["language"], "tr");
    assert_eq!(v["monitors"][0]["status"], "down");
    assert_eq!(v["monitors"][0]["uptime"]["h24"], 0.5);
    assert_eq!(v["monitors"][0]["history"].as_array().unwrap().len(), 2);
    assert_eq!(v["incidents"][0]["ongoing"], true);

    let resp = reqwest::get(format!("{base}/")).await.unwrap();
    let csp = resp.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(csp.contains("default-src 'none'"));
    let html = resp.text().await.unwrap();
    assert!(html.contains("<html lang=\"tr\">"));
    assert!(html.contains("Durum &lt;Sayfası&gt;"), "title is escaped");
    assert!(html.contains("Bazı sistemlere erişilemiyor"));
    assert!(html.contains("Son kesintiler"));
    assert!(html.contains("Devam ediyor"));
    assert!(html.contains("50.00%"));
    assert!(html.contains("<svg class=\"spark\""));
}

#[tokio::test]
async fn assets_are_embedded() {
    let (base, _) = start(ONE).await;
    for (path, ctype) in [
        ("/assets/style.css", "text/css"),
        ("/assets/app.js", "text/javascript"),
        ("/favicon.svg", "image/svg+xml"),
    ] {
        let resp = reqwest::get(format!("{base}{path}")).await.unwrap();
        assert_eq!(resp.status(), 200, "{path}");
        assert!(resp.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with(ctype));
        assert!(!resp.text().await.unwrap().is_empty());
    }
    let resp = reqwest::get(format!("{base}/nope")).await.unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn basic_auth_protects_page_and_api_but_not_healthz() {
    let (base, _) = start(&format!(
        "[web.auth]\nusername = \"admin\"\npassword = \"secret\"\n{ONE}"
    ))
    .await;
    let client = reqwest::Client::new();
    for path in ["/", "/api/status", "/assets/style.css"] {
        let resp = client.get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(resp.status(), 401, "{path}");
        assert!(resp.headers().contains_key("www-authenticate"));
        let resp = client
            .get(format!("{base}{path}"))
            .basic_auth("admin", Some("wrong"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 401, "{path}");
        let resp = client
            .get(format!("{base}{path}"))
            .basic_auth("admin", Some("secret"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "{path}");
    }
    let resp = client.get(format!("{base}/healthz")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
}
