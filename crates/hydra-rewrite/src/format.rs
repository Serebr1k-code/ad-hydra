use anyhow::{bail, ensure};
use regex::bytes::{Regex, RegexBuilder};
use regex_syntax::hir::{Class, Hir, HirKind};
use std::ops::Range;
#[derive(Clone)]
pub struct Format {
    regex: Regex,
    positions: Vec<Vec<u8>>,
}
impl Format {
    pub fn compile(pattern: &str) -> anyhow::Result<Self> {
        ensure!(
            !pattern.is_empty() && pattern.len() <= 4096,
            "flag pattern must contain 1..4096 bytes"
        );
        let hir = regex_syntax::ParserBuilder::new()
            .utf8(false)
            .unicode(false)
            .build()
            .parse(pattern)?;
        let mut positions = Vec::new();
        expand(&hir, &mut positions)?;
        ensure!(
            !positions.is_empty()
                && positions.len() <= 256
                && positions.iter().any(|p| p.len() > 1),
            "flag format must have a variable part and fixed width 1..256"
        );
        let regex = RegexBuilder::new(pattern)
            .unicode(false)
            .size_limit(1024 * 1024)
            .build()?;
        Ok(Self { regex, positions })
    }
    pub fn ranges(&self, input: &[u8]) -> Vec<Range<usize>> {
        self.regex.find_iter(input).map(|m| m.range()).collect()
    }
    pub fn accepts(&self, input: &[u8]) -> bool {
        input.len() == self.positions.len()
            && self
                .regex
                .find(input)
                .is_some_and(|m| m.start() == 0 && m.end() == input.len())
    }
    pub fn generate(&self) -> anyhow::Result<Vec<u8>> {
        let mut output = Vec::with_capacity(self.positions.len());
        for alphabet in &self.positions {
            let random = uuid::Uuid::new_v4().as_u128();
            output.push(alphabet[(random % alphabet.len() as u128) as usize]);
        }
        ensure!(
            self.accepts(&output),
            "generated flag does not satisfy the configured pattern"
        );
        Ok(output)
    }
}
fn expand(h: &Hir, out: &mut Vec<Vec<u8>>) -> anyhow::Result<()> {
    ensure!(out.len() <= 256, "flag format exceeds 256 bytes");
    match h.kind() {
        HirKind::Empty | HirKind::Look(_) => {}
        HirKind::Literal(l) => {
            for b in &l.0 {
                ensure!(
                    (32..=126).contains(b),
                    "flag format must be printable ASCII"
                );
                out.push(vec![*b]);
            }
        }
        HirKind::Class(class) => {
            let mut alphabet = Vec::new();
            match class {
                Class::Bytes(c) => {
                    for r in c.iter() {
                        ensure!(
                            r.start() >= 32 && r.end() <= 126,
                            "flag class must be printable ASCII"
                        );
                        alphabet.extend(r.start()..=r.end());
                    }
                }
                Class::Unicode(c) => {
                    for r in c.iter() {
                        ensure!(
                            (r.start() as u32) >= 32 && (r.end() as u32) <= 126,
                            "flag class must be printable ASCII"
                        );
                        alphabet.extend(r.start() as u8..=r.end() as u8);
                    }
                }
            }
            ensure!(!alphabet.is_empty(), "empty flag character class");
            out.push(alphabet);
        }
        HirKind::Capture(c) => expand(&c.sub, out)?,
        HirKind::Concat(items) => {
            for item in items {
                expand(item, out)?;
            }
        }
        HirKind::Repetition(r) => {
            ensure!(
                r.max == Some(r.min) && r.min <= 256,
                "flag repeats must have exact width at most 256"
            );
            for _ in 0..r.min {
                expand(&r.sub, out)?;
            }
        }
        HirKind::Alternation(_) => {
            bail!("flag generation does not support alternation; use fixed-width character classes")
        }
    }
    ensure!(out.len() <= 256, "flag format exceeds 256 bytes");
    Ok(())
}
