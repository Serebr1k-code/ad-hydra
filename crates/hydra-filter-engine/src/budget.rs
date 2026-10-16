//! Allocation-free preflight runs across the complete snapshot before compiling any regex.
use crate::*;
#[derive(Default)]
struct Budget {
    nodes: usize,
    regexes: usize,
    expected_bytes: usize,
}
pub(crate) fn validate(snapshot: &RuleSnapshot) -> Result<(), CompileError> {
    let mut budget = Budget::default();
    for rule in &snapshot.rules {
        budget.visit(&rule.expression, 1, &mut 0)?;
    }
    Ok(())
}
impl Budget {
    fn visit(
        &mut self,
        expression: &Expression,
        depth: usize,
        rule_nodes: &mut usize,
    ) -> Result<(), CompileError> {
        self.nodes += 1;
        *rule_nodes += 1;
        if depth > MAX_DEPTH || *rule_nodes > MAX_NODES {
            return Err(invalid("expression exceeds depth/node limit"));
        }
        if self.nodes > MAX_SNAPSHOT_NODES {
            return Err(invalid("snapshot node count exceeds 512"));
        }
        match expression {
            Expression::All { all } | Expression::Any { any: all } => {
                for child in all {
                    self.visit(child, depth + 1, rule_nodes)?;
                }
            }
            Expression::Not { not } => self.visit(not, depth + 1, rule_nodes)?,
            Expression::Predicate { op, value, .. } => {
                if *op == Comparison::Regex {
                    self.regexes += 1;
                    if self.regexes > MAX_SNAPSHOT_REGEXES {
                        return Err(invalid("snapshot regex count exceeds 16"));
                    }
                }
                let mut size = 0;
                if !expression::bounded(value, 1, &mut size) {
                    return Err(invalid("expected value exceeds limits"));
                }
                self.expected_bytes += size;
                if self.expected_bytes > MAX_SNAPSHOT_EXPECTED_BYTES {
                    return Err(invalid("snapshot expected value budget exceeds 256 KiB"));
                }
            }
        }
        Ok(())
    }
}
