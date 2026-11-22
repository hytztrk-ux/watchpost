//! Probes against real local listeners.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use chrono::Datelike;
use common::{closed_port, monitor, Target};
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use watchpost::monitor::{Probe, Status};

async fn check(cfg: &str) -> watchpost::monitor::CheckResult {
    let m = monitor(cfg);
    Probe::new(&m).expect("probe").check().await
}

fn http_cfg(url: &str, extra: &str) -> String {
    format!("[[monitor]]\nname = \"t\"\ntype = \"http\"\nurl = \"{url}\"\n{extra}\n")
}

#[tokio::test]
async fn http_up() {
    let t = Target::start().await;
    let r = check(&http_cfg(&t.url("/ok"), "")).await;
    assert_eq!(r.status, Status::Up, "{r:?}");
    assert_eq!(r.message, "HTTP 200");
    assert!(r.latency.is_some());
}

#[tokio::test]
async fn http_unexpected_status_is_down() {
    let t = Target::start().await;
    let r = check(&http_cfg(&t.url("/fail"), "")).await;
    assert_eq!(r.status, Status::Down);
    assert_eq!(r.message, "HTTP 503 (expected 2xx)");
}

#[tokio::test]
async fn http_expected_status_set() {
    let t = Target::start().await;
    let r = check(&http_cfg(
        &t.url("/fail"),
        "expected_status = [200, \"500-503\"]",
    ))
    .await;
    assert_eq!(r.status, Status::Up, "{r:?}");
}

#[tokio::test]
async fn http_keyword() {
    let t = Target::start().await;
    let r = check(&http_cfg(&t.url("/ok"), "keyword = \"watchpost\"")).await;
    assert_eq!(r.status, Status::Up, "{r:?}");
    let r = check(&http_cfg(&t.url("/ok"), "keyword = \"missing-word\"")).await;
    assert_eq!(r.status, Status::Down);
    assert!(r.message.contains("not found"), "{}", r.message);
}

#[tokio::test]
async fn http_method_and_headers_are_sent() {
    let t = Target::start().await;
    let r = check(&http_cfg(
        &t.url("/echo"),
        "method = \"post\"\nbody = \"x\"\nheaders = { \"X-Test\" = \"hi\" }\nkeyword = \"POST hi\"",
    ))
    .await;
    assert_eq!(r.status, Status::Up, "{r:?}");
}

#[tokio::test]
async fn http_redirects() {
    let t = Target::start().await;
    let r = check(&http_cfg(&t.url("/redirect"), "")).await;
    assert_eq!(r.status, Status::Up, "follows redirects by default: {r:?}");
    let r = check(&http_cfg(&t.url("/redirect"), "follow_redirects = false")).await;
    assert_eq!(r.status, Status::Down);
    assert!(r.message.starts_with("HTTP 30"), "{}", r.message);
}

#[tokio::test]
async fn http_timeout_is_down() {
    let t = Target::start().await;
    let started = std::time::Instant::now();
    let r = check(&http_cfg(&t.url("/slow"), "timeout = \"500ms\"")).await;
    assert_eq!(r.status, Status::Down);
    assert!(r.message.contains("timeout"), "{}", r.message);
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn http_connection_refused_is_down() {
    let port = closed_port().await;
    let r = check(&http_cfg(&format!("http://127.0.0.1:{port}/"), "")).await;
    assert_eq!(r.status, Status::Down);
    assert!(r.message.contains("refused"), "{}", r.message);
}

#[tokio::test]
async fn tcp_up_and_down() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let cfg =
        |p: u16| format!("[[monitor]]\nname=\"t\"\ntype=\"tcp\"\nhost=\"127.0.0.1\"\nport={p}\n");

    let r = check(&cfg(port)).await;
    assert_eq!(r.status, Status::Up, "{r:?}");
    assert_eq!(r.message, "connected");

    drop(listener);
    let r = check(&cfg(port)).await;
    assert_eq!(r.status, Status::Down);
    assert_eq!(r.message, "connection refused");
}

// --- TLS -------------------------------------------------------------------

struct TestPki {
    ca_pem: String,
    leaf: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
}

/// Set `$field` to midnight UTC of the day `$days` from today.
macro_rules! set_date {
    ($field:expr, $days:expr) => {{
        let d = chrono::Utc::now() + chrono::Duration::days($days);
        $field = rcgen::date_time_ymd(d.year(), d.month() as u8, d.day() as u8);
    }};
}

fn make_pki(not_after_days: i64) -> TestPki {
    let ca_key = KeyPair::generate().unwrap();
    let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.distinguished_name
        .push(DnType::CommonName, "watchpost test CA");
    ca.not_before = rcgen::date_time_ymd(2020, 1, 1);
    ca.not_after = rcgen::date_time_ymd(2090, 1, 1);
    let ca_cert = ca.self_signed(&ca_key).unwrap();

    let leaf_key = KeyPair::generate().unwrap();
    let mut leaf = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    leaf.distinguished_name
        .push(DnType::CommonName, "localhost");
    leaf.not_before = rcgen::date_time_ymd(2020, 1, 1);
    set_date!(leaf.not_after, not_after_days);
    let leaf_cert = leaf.signed_by(&leaf_key, &ca_cert, &ca_key).unwrap();

    TestPki {
        ca_pem: ca_cert.pem(),
        leaf: leaf_cert.der().clone(),
        key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der())),
    }
}

async fn tls_server(pki: &TestPki) -> SocketAddr {
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![pki.leaf.clone()], pki.key.clone_key())
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(s) = acceptor.accept(stream).await {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    drop(s);
                }
            });
        }
    });
    addr
}

fn tls_cfg(addr: SocketAddr, extra: &str) -> String {
    format!(
        "[[monitor]]\nname=\"t\"\ntype=\"tls\"\nhost=\"127.0.0.1\"\nport={}\nserver_name=\"localhost\"\nwarn_days=14\n{extra}\n",
        addr.port()
    )
}

fn ca_file(pki: &TestPki) -> tempfile::NamedTempFile {
    let f = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(f.path(), &pki.ca_pem).unwrap();
    f
}

#[tokio::test]
async fn tls_valid_certificate_with_private_ca_is_up() {
    let pki = make_pki(100);
    let addr = tls_server(&pki).await;
    let ca = ca_file(&pki);
    let extra = format!("ca_file = '{}'", ca.path().display());
    let r = check(&tls_cfg(addr, &extra)).await;
    assert_eq!(r.status, Status::Up, "{r:?}");
    let days = r.cert_days_left.unwrap();
    assert!((98..=100).contains(&days), "{days}");
}

#[tokio::test]
async fn tls_untrusted_certificate_is_down() {
    let pki = make_pki(100);
    let addr = tls_server(&pki).await;
    let r = check(&tls_cfg(addr, "")).await;
    assert_eq!(r.status, Status::Down, "{r:?}");
    assert!(r.message.contains("not trusted"), "{}", r.message);
    // The expiry date is still read.
    assert!(r.cert_days_left.unwrap() > 90);
}

#[tokio::test]
async fn tls_expiring_certificate_warns() {
    let pki = make_pki(5);
    let addr = tls_server(&pki).await;
    let r = check(&tls_cfg(addr, "skip_verify = true")).await;
    assert_eq!(r.status, Status::Warn, "{r:?}");
    let days = r.cert_days_left.unwrap();
    assert!((3..=5).contains(&days), "{days}");
    assert!(r.message.contains("expires in"), "{}", r.message);
}

#[tokio::test]
async fn tls_expired_certificate_is_down() {
    let pki = make_pki(-3);
    let addr = tls_server(&pki).await;
    let r = check(&tls_cfg(addr, "skip_verify = true")).await;
    assert_eq!(r.status, Status::Down, "{r:?}");
    assert!(r.message.contains("expired"), "{}", r.message);
}

#[tokio::test]
async fn tls_connection_refused_is_down() {
    let port = closed_port().await;
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let r = check(&tls_cfg(addr, "")).await;
    assert_eq!(r.status, Status::Down);
    assert_eq!(r.message, "connection refused");
}

#[tokio::test]
async fn tls_on_plain_tcp_port_is_down() {
    // A plain HTTP server is not a TLS server.
    let t = Target::start().await;
    let r = check(&tls_cfg(t.addr, "timeout = \"2s\"")).await;
    assert_eq!(r.status, Status::Down);
    assert!(r.message.contains("TLS handshake failed"), "{}", r.message);
}

#[tokio::test]
async fn bad_ca_file_fails_probe_creation() {
    let f = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(f.path(), "not a certificate").unwrap();
    let m = monitor(&tls_cfg(
        "127.0.0.1:1".parse().unwrap(),
        &format!("ca_file = '{}'", f.path().display()),
    ));
    let err = Probe::new(&m).err().expect("must fail");
    assert!(err.to_string().contains("ca_file"), "{err}");
}
