//! Constant-space aggregate budgets; no client identity or denial event allocation.
use hydra_domain::ServiceProtection;
use hydra_wire::v1;
use std::{
    sync::{Arc, Mutex},
    time::Instant,
};
#[derive(Default)]
struct Bucket {
    tokens: f64,
    last: Option<Instant>,
    rate: u64,
}
impl Bucket {
    fn update(&mut self, rate: u64, now: Instant) {
        // Concurrent callers may obtain timestamps before acquiring the budget lock.
        let now = self.last.map_or(now, |last| now.max(last));
        if let Some(last) = self.last {
            self.tokens = (self.tokens + now.duration_since(last).as_secs_f64() * self.rate as f64)
                .min(rate as f64);
        } else {
            self.tokens = rate as f64;
        }
        self.rate = rate;
        self.last = Some(now);
    }
    fn available(&self, amount: u64) -> bool {
        self.rate == 0 || amount as f64 <= self.tokens
    }
    fn deduct(&mut self, amount: u64) {
        if self.rate != 0 {
            self.tokens -= amount as f64;
        }
    }
    fn take(&mut self, amount: u64, now: Instant) -> bool {
        self.update(self.rate, now);
        if !self.available(amount) {
            return false;
        }
        self.deduct(amount);
        true
    }
}
#[derive(Default)]
struct Inner {
    config: Option<ServiceProtection>,
    tcp: Bucket,
    udp: Bucket,
    bytes: Bucket,
    metrics: v1::ServiceMetrics,
}
pub struct Budget(Mutex<Inner>);
impl Budget {
    pub fn new(id: &str) -> Arc<Self> {
        Arc::new(Self(Mutex::new(Inner {
            metrics: v1::ServiceMetrics {
                service_id: id.into(),
                ..Default::default()
            },
            ..Default::default()
        })))
    }
    pub fn configure(&self, config: Option<ServiceProtection>) {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        s.tcp.update(
            config.as_ref().map_or(0, |p| p.tcp_connections_per_second),
            now,
        );
        s.udp.update(
            config.as_ref().map_or(0, |p| p.udp_datagrams_per_second),
            now,
        );
        s.bytes
            .update(config.as_ref().map_or(0, |p| p.bytes_per_second), now);
        s.config = config;
    }
    pub fn admit(self: &Arc<Self>, tcp: bool) -> Option<Lease> {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let full = s
            .config
            .as_ref()
            .is_some_and(|p| s.metrics.active_flows >= p.max_active_flows);
        let limited = !full && tcp && !s.tcp.take(1, Instant::now());
        if full || limited {
            s.metrics.rejected_flows += 1;
            if limited {
                s.metrics.rate_limited += 1;
            }
            return None;
        }
        s.metrics.active_flows += 1;
        s.metrics.accepted_flows += 1;
        Some(Lease(self.clone()))
    }
    pub fn reject(&self) {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        s.metrics.rejected_flows += 1;
    }
    pub fn transfer(&self, bytes: usize, datagram: bool) -> bool {
        self.transfer_at(bytes, datagram, Instant::now())
    }
    fn transfer_at(&self, bytes: usize, datagram: bool, now: Instant) -> bool {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let udp_rate = s.udp.rate;
        let byte_rate = s.bytes.rate;
        s.udp.update(udp_rate, now);
        s.bytes.update(byte_rate, now);
        if (datagram && !s.udp.available(1)) || !s.bytes.available(bytes as u64) {
            s.metrics.rate_limited += 1;
            if !datagram {
                s.metrics.rejected_flows += 1;
            }
            return false;
        }
        if datagram {
            s.udp.deduct(1);
        }
        s.bytes.deduct(bytes as u64);
        true
    }
    pub fn dropped_datagram(&self) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .metrics
            .dropped_datagrams += 1;
    }
    pub fn forwarded(&self, bytes: usize, up: bool) {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if up {
            s.metrics.bytes_client_to_server += bytes as u64;
        } else {
            s.metrics.bytes_server_to_client += bytes as u64;
        }
    }
    pub fn idle(&self, default: u64) -> u64 {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .config
            .as_ref()
            .map_or(default, |p| p.idle_timeout_ms)
    }
    pub fn metrics(&self) -> v1::ServiceMetrics {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .metrics
            .clone()
    }
}
pub struct Lease(pub Arc<Budget>);
impl Drop for Lease {
    fn drop(&mut self) {
        self.0
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .metrics
            .active_flows -= 1;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> ServiceProtection {
        ServiceProtection {
            max_active_flows: 1,
            tcp_connections_per_second: 1,
            udp_datagrams_per_second: 1,
            bytes_per_second: 4,
            idle_timeout_ms: 10,
        }
    }
    #[test]
    fn cap_releases_and_survives_config_edits() {
        let b = Budget::new("a");
        let mut config = config();
        config.tcp_connections_per_second = 0;
        b.configure(Some(config.clone()));
        let lease = b.admit(true).unwrap();
        assert!(b.admit(false).is_none());
        b.configure(Some(config));
        assert!(b.admit(true).is_none());
        drop(lease);
        assert!(b.admit(false).is_some());
        assert_eq!(b.metrics().active_flows, 0);
        assert_eq!(b.metrics().accepted_flows, 2);
    }
    #[test]
    fn edits_preserve_remaining_tokens_without_refill() {
        let now = Instant::now();
        let mut b = Bucket::default();
        b.update(4, now);
        assert!(b.take(4, now));
        b.update(8, now);
        assert!(!b.take(1, now));
        b.update(0, now);
        assert!(b.take(u64::MAX, now));
        b.update(4, now);
        assert!(!b.take(1, now));
        b.update(4, now + std::time::Duration::from_secs(1));
        assert!(!b.take(5, now + std::time::Duration::from_secs(1)));
        assert!(b.take(4, now + std::time::Duration::from_secs(1)));
    }
    #[test]
    fn rejected_datagrams_do_not_spend_either_budget() {
        let b = Budget::new("atomic");
        let mut config = config();
        config.udp_datagrams_per_second = 1;
        config.bytes_per_second = 100;
        b.configure(Some(config));
        let now = b.0.lock().unwrap().bytes.last.unwrap();
        assert!(b.transfer_at(0, true, now));
        assert!(!b.transfer_at(100, true, now)); // packet limit rejects without consuming bytes
        assert!(b.transfer_at(100, false, now));
        let later = now + std::time::Duration::from_secs(1);
        assert!(!b.transfer_at(101, true, later)); // byte burst rejects without consuming a packet
        assert!(b.transfer_at(0, true, later));
        assert!(b.transfer_at(100, false, later));
    }
    #[test]
    fn zero_rates_do_not_consume_tokens() {
        for (packets, bytes) in [(0, 100), (1, 0), (0, 0)] {
            let b = Budget::new("unlimited");
            let mut config = config();
            config.udp_datagrams_per_second = packets;
            config.bytes_per_second = bytes;
            b.configure(Some(config));
            let now = b.0.lock().unwrap().bytes.last.unwrap();
            assert!(b.transfer_at(100, true, now));
            if packets == 0 && bytes == 0 {
                assert!(b.transfer_at(usize::MAX, true, now));
            }
            let s = b.0.lock().unwrap();
            assert!(s.bytes.tokens >= 0.0);
            assert!(s.udp.tokens >= 0.0);
        }
    }
    #[test]
    fn refill_is_deterministic() {
        let now = Instant::now();
        let mut b = Bucket::default();
        b.update(2, now);
        assert!(b.take(2, now));
        b.update(2, now + std::time::Duration::from_millis(500));
        assert!(b.take(1, now + std::time::Duration::from_millis(500)));
        assert!(!b.take(1, now + std::time::Duration::from_millis(500)));
    }
}
