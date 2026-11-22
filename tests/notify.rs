//! Notification channels against local mock servers.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{config, Recorder};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use watchpost::alert::EventKind;
use watchpost::monitor::Status;
use watchpost::notify::{Dispatcher, MessageContext, Notification};

fn down_event() -> Notification {
    Notification {
        kind: EventKind::Down,
        status: Status::Down,
        monitor: "API".into(),
        monitor_type: "http".into(),
        target: "http://10.0.0.1/health".into(),
        message: "connection refused".into(),
        at: 1_700_000_000,
        down_since: Some(1_700_000_000),
    }
}

fn dispatcher(notifiers: &str, delays: Vec<Duration>) -> Arc<Dispatcher> {
    let text = format!(
        "title = \"Prod\"\n{notifiers}\n[[monitor]]\nname=\"x\"\ntype=\"tcp\"\nhost=\"localhost\"\nport=1\n"
    );
    let cfg = config(&text);
    Dispatcher::with_notifiers(&cfg.notifiers, MessageContext::from_config(&cfg), delays).unwrap()
}

#[tokio::test]
async fn telegram_sends_message() {
    let mock = Recorder::start().await;
    let d = dispatcher(
        &format!(
            "[[notifier]]\nname=\"tg\"\ntype=\"telegram\"\nbot_token=\"123:abc\"\nchat_id=-42\napi_url=\"{}\"\n",
            mock.url("")
        ),
        vec![],
    );
    d.send_to("tg", &down_event()).await.unwrap();
    let reqs = mock.requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].method, "POST");
    assert_eq!(reqs[0].path, "/bot123:abc/sendMessage");
    let body = reqs[0].json();
    assert_eq!(body["chat_id"], "-42");
    let text = body["text"].as_str().unwrap();
    assert!(text.contains("[Prod] DOWN: API"), "{text}");
    assert!(text.contains("connection refused"), "{text}");
}

#[tokio::test]
async fn discord_sends_content() {
    let mock = Recorder::start_with(0, 204).await;
    let d = dispatcher(
        &format!(
            "[[notifier]]\ntype=\"discord\"\nwebhook_url=\"{}\"\nusername=\"watchpost\"\n",
            mock.url("/api/webhooks/1/secret")
        ),
        vec![],
    );
    d.send_to("discord", &down_event()).await.unwrap();
    let reqs = mock.requests();
    assert_eq!(reqs[0].path, "/api/webhooks/1/secret");
    let body = reqs[0].json();
    assert_eq!(body["username"], "watchpost");
    assert!(body["content"].as_str().unwrap().contains("DOWN: API"));
}

#[tokio::test]
async fn webhook_posts_structured_json() {
    let mock = Recorder::start().await;
    let d = dispatcher(
        &format!(
            "[[notifier]]\nname=\"hook\"\ntype=\"webhook\"\nurl=\"{}\"\nheaders={{ Authorization = \"Bearer t0k\" }}\n",
            mock.url("/hook")
        ),
        vec![],
    );
    d.send_to("hook", &down_event()).await.unwrap();
    let reqs = mock.requests();
    assert_eq!(reqs[0].path, "/hook");
    assert_eq!(reqs[0].headers["authorization"], "Bearer t0k");
    assert_eq!(reqs[0].headers["content-type"], "application/json");
    let body = reqs[0].json();
    assert_eq!(body["event"], "down");
    assert_eq!(body["status"], "down");
    assert_eq!(body["monitor"], "API");
    assert_eq!(body["type"], "http");
    assert_eq!(body["target"], "http://10.0.0.1/health");
    assert_eq!(body["message"], "connection refused");
    assert_eq!(body["timestamp"], "2023-11-14T22:13:20Z");
    assert_eq!(body["unix_time"], 1_700_000_000);
    assert_eq!(body["title"], "Prod");
    assert!(body["text"].as_str().unwrap().contains("DOWN"));
}

#[tokio::test]
async fn error_responses_are_reported_without_secrets() {
    let mock = Recorder::start_with(0, 401).await;
    let d = dispatcher(
        &format!(
            "[[notifier]]\nname=\"tg\"\ntype=\"telegram\"\nbot_token=\"999:supersecret\"\nchat_id=1\napi_url=\"{}\"\n",
            mock.url("")
        ),
        vec![],
    );
    let err = d
        .send_to("tg", &down_event())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("401"), "{err}");
    assert!(!err.contains("supersecret"), "{err}");
}

#[tokio::test]
async fn connection_errors_do_not_leak_urls() {
    let port = common::closed_port().await;
    let d = dispatcher(
        &format!(
            "[[notifier]]\nname=\"tg\"\ntype=\"telegram\"\nbot_token=\"999:supersecret\"\nchat_id=1\napi_url=\"http://127.0.0.1:{port}\"\n"
        ),
        vec![],
    );
    let err = d
        .send_to("tg", &down_event())
        .await
        .unwrap_err()
        .to_string();
    assert!(!err.contains("supersecret"), "{err}");
}

#[tokio::test]
async fn dispatch_retries_failed_deliveries() {
    let mock = Recorder::start_with(2, 200).await;
    let d = dispatcher(
        &format!(
            "[[notifier]]\nname=\"hook\"\ntype=\"webhook\"\nurl=\"{}\"\n",
            mock.url("/hook")
        ),
        vec![Duration::from_millis(50), Duration::from_millis(50)],
    );
    d.dispatch(down_event(), &["hook".to_string()]);
    d.shutdown(Duration::from_secs(5)).await;
    assert_eq!(mock.requests().len(), 3, "two failures, then success");
}

#[tokio::test]
async fn dispatch_gives_up_after_retries() {
    let mock = Recorder::start_with(0, 500).await;
    let d = dispatcher(
        &format!(
            "[[notifier]]\nname=\"hook\"\ntype=\"webhook\"\nurl=\"{}\"\n",
            mock.url("/hook")
        ),
        vec![Duration::from_millis(20)],
    );
    d.dispatch(down_event(), &["hook".to_string()]);
    d.shutdown(Duration::from_secs(5)).await;
    assert_eq!(mock.requests().len(), 2);
}

#[tokio::test]
async fn dispatch_only_to_selected_notifiers() {
    let a = Recorder::start().await;
    let b = Recorder::start().await;
    let d = dispatcher(
        &format!(
            "[[notifier]]\nname=\"a\"\ntype=\"webhook\"\nurl=\"{}\"\n[[notifier]]\nname=\"b\"\ntype=\"webhook\"\nurl=\"{}\"\n",
            a.url("/"),
            b.url("/")
        ),
        vec![],
    );
    d.dispatch(down_event(), &["b".to_string()]);
    d.shutdown(Duration::from_secs(5)).await;
    assert_eq!(a.requests().len(), 0);
    assert_eq!(b.requests().len(), 1);
}

#[tokio::test]
async fn unknown_notifier_is_an_error() {
    let d = dispatcher("", vec![]);
    assert!(d.send_to("nope", &down_event()).await.is_err());
}

/// Minimal SMTP server: no TLS, no auth; records the DATA payload.
async fn fake_smtp() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mails = Arc::new(Mutex::new(Vec::new()));
    let store = mails.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let store = store.clone();
            tokio::spawn(async move {
                let (r, mut w) = stream.into_split();
                let mut lines = BufReader::new(r).lines();
                w.write_all(b"220 localhost ESMTP fake\r\n").await.unwrap();
                let mut envelope = String::new();
                while let Ok(Some(line)) = lines.next_line().await {
                    let upper = line.to_ascii_uppercase();
                    if upper.starts_with("EHLO") || upper.starts_with("HELO") {
                        w.write_all(b"250-localhost\r\n250 8BITMIME\r\n")
                            .await
                            .unwrap();
                    } else if upper.starts_with("MAIL FROM") || upper.starts_with("RCPT TO") {
                        envelope.push_str(&line);
                        envelope.push('\n');
                        w.write_all(b"250 OK\r\n").await.unwrap();
                    } else if upper == "DATA" {
                        w.write_all(b"354 go ahead\r\n").await.unwrap();
                        let mut data = envelope.clone();
                        while let Ok(Some(l)) = lines.next_line().await {
                            if l == "." {
                                break;
                            }
                            data.push_str(&l);
                            data.push('\n');
                        }
                        store.lock().unwrap().push(data);
                        w.write_all(b"250 queued\r\n").await.unwrap();
                    } else if upper == "QUIT" {
                        w.write_all(b"221 bye\r\n").await.unwrap();
                        return;
                    } else {
                        w.write_all(b"250 OK\r\n").await.unwrap();
                    }
                }
            });
        }
    });
    (port, mails)
}

#[tokio::test]
async fn email_is_delivered_over_smtp() {
    let (port, mails) = fake_smtp().await;
    let d = dispatcher(
        &format!(
            "[[notifier]]\nname=\"mail\"\ntype=\"email\"\nhost=\"127.0.0.1\"\nport={port}\ntls=\"none\"\nfrom=\"watchpost <alerts@example.com>\"\nto=[\"ops@example.com\", \"oncall@example.com\"]\n"
        ),
        vec![],
    );
    d.send_to("mail", &down_event()).await.unwrap();
    let mails = mails.lock().unwrap().clone();
    assert_eq!(mails.len(), 1);
    let m = &mails[0];
    assert!(m.contains("MAIL FROM:<alerts@example.com>"), "{m}");
    assert!(m.contains("RCPT TO:<ops@example.com>"), "{m}");
    assert!(m.contains("RCPT TO:<oncall@example.com>"), "{m}");
    assert!(m.contains("Subject: [Prod] DOWN: API"), "{m}");
    assert!(m.contains("connection refused"), "{m}");
}

#[tokio::test]
async fn email_connection_failure_is_an_error() {
    let port = common::closed_port().await;
    let d = dispatcher(
        &format!(
            "[[notifier]]\nname=\"mail\"\ntype=\"email\"\nhost=\"127.0.0.1\"\nport={port}\ntls=\"none\"\nfrom=\"a@example.com\"\nto=\"b@example.com\"\n"
        ),
        vec![],
    );
    let err = d.send_to("mail", &down_event()).await.unwrap_err();
    assert!(err.to_string().starts_with("email:"), "{err}");
}
