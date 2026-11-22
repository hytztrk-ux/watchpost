use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, RootCertStore, SignatureScheme};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use super::{describe_io_error, error_chain, format_timeout, CheckResult, ProbeError, Status};
use crate::config::{Monitor, TlsMonitor};

const SECS_PER_DAY: i64 = 86_400;

pub struct TlsProbe {
    host: String,
    port: u16,
    server_name: ServerName<'static>,
    warn_days: u32,
    skip_verify: bool,
    timeout: Duration,
    provider: Arc<CryptoProvider>,
    verifier: Arc<WebPkiServerVerifier>,
}

impl TlsProbe {
    pub fn new(monitor: &Monitor, cfg: &TlsMonitor) -> Result<Self, ProbeError> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        if let Some(path) = &cfg.ca_file {
            let ca_err = |message: String| ProbeError::CaFile {
                monitor: monitor.name.clone(),
                path: path.display().to_string(),
                message,
            };
            let certs = CertificateDer::pem_file_iter(path)
                .map_err(|e| ca_err(e.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| ca_err(e.to_string()))?;
            if certs.is_empty() {
                return Err(ca_err("no certificates found".into()));
            }
            for cert in certs {
                roots.add(cert).map_err(|e| ca_err(e.to_string()))?;
            }
        }
        let verifier =
            WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
                .build()
                .map_err(|e| ProbeError::Tls {
                    monitor: monitor.name.clone(),
                    message: e.to_string(),
                })?;
        let sni = cfg.server_name.clone().unwrap_or_else(|| cfg.host.clone());
        let server_name = ServerName::try_from(sni).map_err(|e| ProbeError::Tls {
            monitor: monitor.name.clone(),
            message: e.to_string(),
        })?;
        Ok(Self {
            host: cfg.host.clone(),
            port: cfg.port,
            server_name,
            warn_days: cfg.warn_days,
            skip_verify: cfg.skip_verify,
            timeout: monitor.timeout,
            provider,
            verifier,
        })
    }

    pub async fn check(&self) -> CheckResult {
        let start = Instant::now();
        match tokio::time::timeout(self.timeout, self.handshake()).await {
            Ok(Ok((not_after, verify_error))) => {
                let latency = start.elapsed();
                evaluate(
                    not_after,
                    chrono::Utc::now().timestamp(),
                    verify_error.filter(|_| !self.skip_verify),
                    self.warn_days,
                    latency,
                )
            }
            Ok(Err(message)) => CheckResult::down(Some(start.elapsed()), message),
            Err(_) => CheckResult::down(Some(start.elapsed()), format_timeout(self.timeout)),
        }
    }

    /// Connect, complete a handshake and return the leaf certificate's
    /// `notAfter` (unix seconds) plus the chain verification error, if any.
    async fn handshake(&self) -> Result<(i64, Option<rustls::Error>), String> {
        let capture = Arc::new(CapturingVerifier {
            inner: self.verifier.clone(),
            error: Mutex::new(None),
        });
        let config = rustls::ClientConfig::builder_with_provider(self.provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .dangerous()
            .with_custom_certificate_verifier(capture.clone())
            .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(config));

        let tcp = TcpStream::connect((self.host.as_str(), self.port))
            .await
            .map_err(|e| describe_io_error(&e))?;
        let stream = connector
            .connect(self.server_name.clone(), tcp)
            .await
            .map_err(|e| format!("TLS handshake failed: {}", error_chain(&e)))?;

        let (_, conn) = stream.get_ref();
        let leaf = conn
            .peer_certificates()
            .and_then(|c| c.first())
            .ok_or_else(|| "server sent no certificate".to_owned())?;
        let (_, cert) = x509_parser::parse_x509_certificate(leaf.as_ref())
            .map_err(|e| format!("cannot parse certificate: {e}"))?;
        let not_after = cert.validity().not_after.timestamp();
        let verify_error = capture.error.lock().map(|mut g| g.take()).unwrap_or(None);
        Ok((not_after, verify_error))
    }
}

/// Decide the check outcome from the certificate's expiry time.
fn evaluate(
    not_after: i64,
    now: i64,
    verify_error: Option<rustls::Error>,
    warn_days: u32,
    latency: Duration,
) -> CheckResult {
    let remaining = not_after - now;
    let days_left = remaining.div_euclid(SECS_PER_DAY);
    let date = chrono::DateTime::from_timestamp(not_after, 0)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default();

    let (status, message) = if remaining <= 0 {
        let ago = (-remaining) / SECS_PER_DAY;
        (
            Status::Down,
            format!("certificate expired on {date} ({ago} days ago)"),
        )
    } else if let Some(err) = verify_error {
        (Status::Down, format!("certificate not trusted: {err}"))
    } else if days_left < i64::from(warn_days) {
        (
            Status::Warn,
            format!("certificate expires in {days_left} days ({date})"),
        )
    } else {
        (
            Status::Up,
            format!("certificate valid for {days_left} days ({date})"),
        )
    };
    CheckResult {
        status,
        latency: Some(latency),
        message,
        cert_days_left: Some(days_left),
    }
}

/// Runs the normal WebPKI verification but records its verdict instead of
/// aborting the handshake, so expiry dates can be read from untrusted or
/// expired certificates too. Handshake signatures are still verified.
#[derive(Debug)]
struct CapturingVerifier {
    inner: Arc<WebPkiServerVerifier>,
    error: Mutex<Option<rustls::Error>>,
}

impl ServerCertVerifier for CapturingVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if let Err(e) = self.inner.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        ) {
            if let Ok(mut slot) = self.error.lock() {
                *slot = Some(e);
            }
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;
    const DAY: i64 = SECS_PER_DAY;

    fn eval(not_after: i64, err: Option<rustls::Error>) -> CheckResult {
        evaluate(not_after, NOW, err, 14, Duration::from_millis(5))
    }

    #[test]
    fn valid_certificate_is_up() {
        let r = eval(NOW + 60 * DAY + 10, None);
        assert_eq!(r.status, Status::Up);
        assert_eq!(r.cert_days_left, Some(60));
    }

    #[test]
    fn soon_expiring_certificate_warns() {
        let r = eval(NOW + 13 * DAY + 10, None);
        assert_eq!(r.status, Status::Warn);
        assert_eq!(r.cert_days_left, Some(13));
        assert!(r.message.contains("13 days"), "{}", r.message);
        // Exactly warn_days left is still fine.
        assert_eq!(eval(NOW + 14 * DAY + 10, None).status, Status::Up);
    }

    #[test]
    fn expired_certificate_is_down() {
        let r = eval(NOW - 3 * DAY, None);
        assert_eq!(r.status, Status::Down);
        assert!(r.message.contains("expired"), "{}", r.message);
        assert!(r.message.contains("3 days ago"), "{}", r.message);
    }

    #[test]
    fn untrusted_certificate_is_down() {
        let err = rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer);
        let r = eval(NOW + 90 * DAY, Some(err));
        assert_eq!(r.status, Status::Down);
        assert!(r.message.contains("not trusted"), "{}", r.message);
        assert_eq!(r.cert_days_left, Some(90));
    }
}
