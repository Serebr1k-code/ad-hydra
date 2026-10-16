//! Bounded defensive flag scanner. Findings are never eligible for submission.
use hydra_domain::FieldValue;
use hydra_protocol_runtime::Direction;
use regex::bytes::{Regex, RegexBuilder};
use std::sync::Arc;
const MAX_INPUT: usize = 65536;
const MAX_FLAG: usize = 256;
const MAX_FINDINGS: usize = 256;
#[derive(Clone)]
pub struct Scanner {
    regex: Arc<Regex>,
}
pub struct FlagSession {
    scanner: Scanner,
    tails: [Vec<u8>; 2],
}
impl Scanner {
    pub fn new(pattern: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !pattern.is_empty() && pattern.len() <= 4096,
            "invalid flag pattern size"
        );
        let regex = RegexBuilder::new(pattern)
            .size_limit(1024 * 1024)
            .dfa_size_limit(1024 * 1024)
            .build()?;
        anyhow::ensure!(
            !regex.is_match(b""),
            "flag regex must not match empty input"
        );
        Ok(Self {
            regex: Arc::new(regex),
        })
    }
    pub fn session(&self) -> FlagSession {
        FlagSession {
            scanner: self.clone(),
            tails: Default::default(),
        }
    }
    pub fn scan(&self, bytes: &[u8]) -> Vec<String> {
        self.matches(&bytes[..bytes.len().min(MAX_INPUT)], 0)
    }
    fn matches(&self, bytes: &[u8], prefix: usize) -> Vec<String> {
        self.regex
            .find_iter(bytes)
            .filter(|m| m.end() > prefix && !m.is_empty() && m.len() <= MAX_FLAG)
            .filter_map(|m| std::str::from_utf8(m.as_bytes()).ok().map(str::to_owned))
            .take(MAX_FINDINGS)
            .collect()
    }
    /// Scan typed values with a shared 64 KiB budget, depth 16, 1024 nodes.
    pub fn scan_fields(
        &self,
        fields: &std::collections::BTreeMap<String, FieldValue>,
    ) -> Vec<String> {
        fn visit(
            value: &FieldValue,
            depth: usize,
            budget: &mut usize,
            nodes: &mut usize,
            scanner: &Scanner,
            out: &mut Vec<String>,
        ) {
            if depth > 16 || *budget == 0 || *nodes == 0 || out.len() >= MAX_FINDINGS {
                return;
            }
            *nodes -= 1;
            let bytes = match value {
                FieldValue::String(v) => Some(v.as_bytes()),
                FieldValue::Bytes(v) => Some(v.as_slice()),
                _ => None,
            };
            if let Some(bytes) = bytes {
                let n = bytes.len().min(*budget);
                *budget -= n;
                out.extend(
                    scanner
                        .scan(&bytes[..n])
                        .into_iter()
                        .take(MAX_FINDINGS - out.len()),
                );
            }
            match value {
                FieldValue::Object(values) => {
                    for v in values.values() {
                        visit(v, depth + 1, budget, nodes, scanner, out)
                    }
                }
                FieldValue::List(values) => {
                    for v in values {
                        visit(v, depth + 1, budget, nodes, scanner, out)
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        let mut budget = MAX_INPUT;
        let mut nodes = 1024;
        for v in fields.values() {
            visit(v, 0, &mut budget, &mut nodes, self, &mut out)
        }
        out
    }
}
impl FlagSession {
    /// Input beyond 64 KiB is intentionally outside this call's scan budget.
    /// Callers must feed bounded transport read chunks (at most 64 KiB).
    pub fn feed(&mut self, direction: Direction, bytes: &[u8]) -> Vec<String> {
        let index = match direction {
            Direction::ClientToServer => 0,
            Direction::ServerToClient => 1,
        };
        let tail = &mut self.tails[index];
        let prefix = tail.len();
        let mut input = Vec::with_capacity(prefix + bytes.len().min(MAX_INPUT));
        input.extend_from_slice(tail);
        input.extend_from_slice(&bytes[..bytes.len().min(MAX_INPUT)]);
        let out = self.scanner.matches(&input, prefix);
        tail.clear();
        tail.extend_from_slice(&input[input.len().saturating_sub(MAX_FLAG - 1)..]);
        out
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_flags_and_overlap_dedup() {
        let mut s = Scanner::new("FLAG[0-9]{4}").unwrap().session();
        assert!(s.feed(Direction::ClientToServer, b"prefixFL").is_empty());
        assert_eq!(
            s.feed(Direction::ClientToServer, b"AG1234"),
            vec!["FLAG1234"]
        );
        assert!(s.feed(Direction::ClientToServer, b"suffix").is_empty());
    }
    #[test]
    fn direction_isolation() {
        let mut s = Scanner::new("FLAG[0-9]{4}").unwrap().session();
        s.feed(Direction::ClientToServer, b"FL");
        assert!(s.feed(Direction::ServerToClient, b"AG1234").is_empty());
    }
    #[test]
    fn bounded_findings_and_tail() {
        let mut s = Scanner::new("FLAG").unwrap().session();
        assert_eq!(
            s.feed(Direction::ClientToServer, &b"FLAG".repeat(20000))
                .len(),
            256
        );
        assert!(s.tails[0].len() <= 255);
        assert!(Scanner::new(".*").is_err());
    }
}
