use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, Url};

use super::{describe_reqwest_error, format_timeout, CheckResult, ProbeError};
use crate::config::{HttpMonitor, Monitor, StatusMatcher};

/// At most this many bytes of the body are read when searching for a keyword.
const MAX_BODY_BYTES: usize = 1024 * 1024;

pub struct HttpProbe {
    client: reqwest::Client,
    method: Method,
    url: Url,
    headers: HeaderMap,
    body: Option<String>,
    expected: Vec<StatusMatcher>,
    keyword: Option<String>,
    timeout: Duration,
}

impl HttpProbe {
    pub fn new(monitor: &Monitor, cfg: &HttpMonitor) -> Result<Self, ProbeError> {
        let redirect = if cfg.follow_redirects {
            reqwest::redirect::Policy::limited(10)
        } else {
            reqwest::redirect::Policy::none()
        };
        let client = reqwest::Client::builder()
            .user_agent(concat!("watchpost/", env!("CARGO_PKG_VERSION")))
            .redirect(redirect)
            .timeout(monitor.timeout)
            .connect_timeout(monitor.timeout)
            // Every check opens a fresh connection: it measures what a real
            // client would see and keeps no idle sockets around.
            .pool_max_idle_per_host(0)
            .danger_accept_invalid_certs(cfg.skip_tls_verify)
            .build()
            .map_err(|source| ProbeError::HttpClient {
                monitor: monitor.name.clone(),
                source,
            })?;

        // These were validated when the config was loaded.
        let mut headers = HeaderMap::new();
        for (k, v) in &cfg.headers {
            if let (Ok(k), Ok(v)) = (
                HeaderName::from_bytes(k.as_bytes()),
                HeaderValue::from_str(v),
            ) {
                headers.insert(k, v);
            }
        }
        let method =
            Method::from_bytes(cfg.method.to_ascii_uppercase().as_bytes()).unwrap_or(Method::GET);
        let url = Url::parse(&cfg.url).expect("url validated at load time");

        Ok(Self {
            client,
            method,
            url,
            headers,
            body: cfg.body.clone(),
            expected: cfg.expected_status.clone(),
            keyword: cfg.keyword.clone(),
            timeout: monitor.timeout,
        })
    }

    pub async fn check(&self) -> CheckResult {
        let start = Instant::now();
        match tokio::time::timeout(self.timeout, self.run(start)).await {
            Ok(result) => result,
            Err(_) => CheckResult::down(Some(start.elapsed()), format_timeout(self.timeout)),
        }
    }

    async fn run(&self, start: Instant) -> CheckResult {
        let mut req = self
            .client
            .request(self.method.clone(), self.url.clone())
            .headers(self.headers.clone());
        if let Some(body) = &self.body {
            req = req.body(body.clone());
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                return CheckResult::down(
                    Some(start.elapsed()),
                    describe_reqwest_error(&e, self.timeout),
                )
            }
        };
        let code = resp.status().as_u16();
        if !self.expected.iter().any(|m| m.matches(code)) {
            let expected: Vec<String> = self.expected.iter().map(|m| m.to_string()).collect();
            return CheckResult::down(
                Some(start.elapsed()),
                format!("HTTP {code} (expected {})", expected.join(", ")),
            );
        }

        if let Some(keyword) = &self.keyword {
            let mut stream = resp.bytes_stream();
            let mut buf: Vec<u8> = Vec::new();
            let needle = keyword.as_bytes();
            let mut found = false;
            while let Some(chunk) = stream.next().await {
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        return CheckResult::down(
                            Some(start.elapsed()),
                            format!(
                                "error reading body: {}",
                                describe_reqwest_error(&e, self.timeout)
                            ),
                        )
                    }
                };
                // Search only the new data plus enough of the old data to
                // catch a match that spans the chunk border.
                let search_from = buf.len().saturating_sub(needle.len().saturating_sub(1));
                buf.extend_from_slice(&chunk);
                if contains(&buf[search_from..], needle) {
                    found = true;
                    break;
                }
                if buf.len() >= MAX_BODY_BYTES {
                    break;
                }
            }
            if !found {
                return CheckResult::down(
                    Some(start.elapsed()),
                    format!("HTTP {code}, keyword \"{keyword}\" not found"),
                );
            }
            return CheckResult::up(start.elapsed(), format!("HTTP {code}, keyword found"));
        }

        CheckResult::up(start.elapsed(), format!("HTTP {code}"))
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::contains;

    #[test]
    fn contains_works() {
        assert!(contains(b"hello world", b"o w"));
        assert!(!contains(b"hello", b"world"));
        assert!(!contains(b"he", b"hello"));
        assert!(contains(b"x", b""));
    }
}
