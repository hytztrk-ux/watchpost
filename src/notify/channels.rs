use std::time::Duration;

use lettre::message::header::ContentType;
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::json;

use super::{format_rfc3339, render, MessageContext, Notification, NotifyError};
use crate::config::{EmailConfig, NotifierKind, SmtpTls};

/// A configured, ready-to-use notification channel.
pub enum Channel {
    Telegram {
        client: reqwest::Client,
        endpoint: String,
        chat_id: String,
    },
    Discord {
        client: reqwest::Client,
        url: String,
        username: Option<String>,
    },
    Webhook {
        client: reqwest::Client,
        url: String,
        headers: HeaderMap,
    },
    Email {
        transport: AsyncSmtpTransport<Tokio1Executor>,
        from: Mailbox,
        to: Vec<Mailbox>,
    },
}

impl Channel {
    pub fn new(kind: &NotifierKind, client: reqwest::Client) -> Result<Channel, NotifyError> {
        Ok(match kind {
            NotifierKind::Telegram(t) => Channel::Telegram {
                client,
                endpoint: format!(
                    "{}/bot{}/sendMessage",
                    t.api_url.trim_end_matches('/'),
                    t.bot_token
                ),
                chat_id: t.chat_id.clone(),
            },
            NotifierKind::Discord(d) => Channel::Discord {
                client,
                url: d.webhook_url.clone(),
                username: d.username.clone(),
            },
            NotifierKind::Webhook(w) => {
                let mut headers = HeaderMap::new();
                for (k, v) in &w.headers {
                    if let (Ok(k), Ok(v)) = (
                        HeaderName::from_bytes(k.as_bytes()),
                        HeaderValue::from_str(v),
                    ) {
                        headers.insert(k, v);
                    }
                }
                Channel::Webhook {
                    client,
                    url: w.url.clone(),
                    headers,
                }
            }
            NotifierKind::Email(e) => email_channel(e)?,
        })
    }

    pub async fn send(&self, n: &Notification, ctx: &MessageContext) -> Result<(), NotifyError> {
        let rendered = render(n, ctx);
        match self {
            Channel::Telegram {
                client,
                endpoint,
                chat_id,
            } => {
                let body = json!({
                    "chat_id": chat_id,
                    "text": rendered.text,
                    "disable_web_page_preview": true,
                });
                check_response(client.post(endpoint).json(&body).send().await?).await
            }
            Channel::Discord {
                client,
                url,
                username,
            } => {
                // Discord limits message content to 2000 characters.
                let content: String = rendered.text.chars().take(2000).collect();
                let mut body = json!({ "content": content });
                if let Some(u) = username {
                    body["username"] = json!(u);
                }
                check_response(client.post(url).json(&body).send().await?).await
            }
            Channel::Webhook {
                client,
                url,
                headers,
            } => {
                let body = json!({
                    "event": n.kind,
                    "status": n.status,
                    "monitor": n.monitor,
                    "type": n.monitor_type,
                    "target": n.target,
                    "message": n.message,
                    "timestamp": format_rfc3339(n.at),
                    "unix_time": n.at,
                    "down_since": n.down_since.map(format_rfc3339),
                    "duration_seconds": n.down_since.map(|s| n.at - s),
                    "title": ctx.title,
                    "subject": rendered.subject,
                    "text": rendered.text,
                    "status_page": ctx.public_url,
                });
                let resp = client
                    .post(url)
                    .headers(headers.clone())
                    .json(&body)
                    .send()
                    .await?;
                check_response(resp).await
            }
            Channel::Email {
                transport,
                from,
                to,
            } => {
                let mut builder = Message::builder()
                    .from(from.clone())
                    .subject(rendered.subject)
                    .header(ContentType::TEXT_PLAIN);
                for m in to {
                    builder = builder.to(m.clone());
                }
                let message = builder
                    .body(rendered.text)
                    .map_err(|e| NotifyError::Email(e.to_string()))?;
                transport
                    .send(message)
                    .await
                    .map_err(|e| NotifyError::Email(crate::monitor::error_chain(&e)))?;
                Ok(())
            }
        }
    }
}

fn email_channel(e: &EmailConfig) -> Result<Channel, NotifyError> {
    let err = |m: String| NotifyError::Email(m);
    let builder = match e.tls {
        SmtpTls::Starttls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&e.host)
            .map_err(|x| err(x.to_string()))?,
        SmtpTls::Tls => {
            AsyncSmtpTransport::<Tokio1Executor>::relay(&e.host).map_err(|x| err(x.to_string()))?
        }
        SmtpTls::None => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&e.host),
    };
    let mut builder = builder
        .port(e.effective_port())
        .timeout(Some(Duration::from_secs(20)));
    if let (Some(user), Some(pass)) = (&e.username, &e.password) {
        builder = builder.credentials(Credentials::new(user.clone(), pass.clone()));
    }
    let from = e
        .from
        .parse::<Mailbox>()
        .map_err(|x| err(format!("invalid from address: {x}")))?;
    let to =
        e.to.iter()
            .map(|t| {
                t.parse::<Mailbox>()
                    .map_err(|x| err(format!("invalid to address `{t}`: {x}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
    Ok(Channel::Email {
        transport: builder.build(),
        from,
        to,
    })
}

async fn check_response(resp: reqwest::Response) -> Result<(), NotifyError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let mut body = resp.text().await.unwrap_or_default();
    if body.len() > 300 {
        let mut cut = 300;
        while !body.is_char_boundary(cut) {
            cut -= 1;
        }
        body.truncate(cut);
        body.push('…');
    }
    Err(NotifyError::Status {
        status: status.as_u16(),
        body,
    })
}
