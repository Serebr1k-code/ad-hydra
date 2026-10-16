//! Fixed-window counters, bounded across the entire published rule snapshot.
use std::collections::BTreeMap;
#[derive(Debug, Default)]
pub struct Windows(BTreeMap<(String, String), (u64, u64, u32)>);
impl Windows {
    /// None means cardinality exhaustion. Existing keys continue normally.
    pub fn exceeds(
        &mut self,
        rule: &str,
        key: String,
        requests: u32,
        window_ms: u64,
        now: u64,
    ) -> Option<bool> {
        let identity = (rule.to_owned(), key);
        if !self.0.contains_key(&identity) && self.0.len() >= 1024 {
            self.0
                .retain(|_, (start, width, _)| now.saturating_sub(*start) < *width);
            if self.0.len() >= 1024 {
                return None;
            }
        }
        let (start, width, count) = self.0.entry(identity).or_insert((now, window_ms, 0));
        if now.saturating_sub(*start) >= *width {
            *start = now;
            *count = 0;
        }
        *count = count.saturating_add(1);
        Some(*count > requests)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cardinality_bound_never_evicts_live_keys_and_reclaims_expired() {
        let mut windows = Windows::default();
        for n in 0..1024 {
            assert_eq!(windows.exceeds("r", n.to_string(), 1, 1000, 0), Some(false));
        }
        assert_eq!(windows.exceeds("r", "new".into(), 1, 1000, 0), None);
        assert_eq!(windows.exceeds("r", "0".into(), 1, 1000, 0), Some(true));
        assert_eq!(
            windows.exceeds("r", "new".into(), 1, 1000, 1000),
            Some(false)
        );
        assert_eq!(windows.0.len(), 1);
    }
}
