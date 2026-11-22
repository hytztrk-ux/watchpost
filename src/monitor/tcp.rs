use std::time::{Duration, Instant};

use tokio::net::TcpStream;

use super::{describe_io_error, format_timeout, CheckResult};
use crate::config::{Monitor, TcpMonitor};

pub struct TcpProbe {
    host: String,
    port: u16,
    timeout: Duration,
}

impl TcpProbe {
    pub fn new(monitor: &Monitor, cfg: &TcpMonitor) -> Self {
        Self {
            host: cfg.host.clone(),
            port: cfg.port,
            timeout: monitor.timeout,
        }
    }

    pub async fn check(&self) -> CheckResult {
        let start = Instant::now();
        let connect = TcpStream::connect((self.host.as_str(), self.port));
        match tokio::time::timeout(self.timeout, connect).await {
            Ok(Ok(stream)) => {
                let latency = start.elapsed();
                drop(stream);
                CheckResult::up(latency, "connected")
            }
            Ok(Err(e)) => CheckResult::down(Some(start.elapsed()), describe_io_error(&e)),
            Err(_) => CheckResult::down(Some(start.elapsed()), format_timeout(self.timeout)),
        }
    }
}
