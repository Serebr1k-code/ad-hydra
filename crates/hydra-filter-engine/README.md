# Bounded filter compilation

Snapshot validation performs an allocation-free aggregate expression walk before
compiling any regex or cloning compiled expressions. The entire snapshot shares
these budgets:

- 512 AST nodes across all rules; 256 nodes/rule and depth 16 still apply.
- 16 regex predicates across all rules.
- 256 KiB total expected-value inspection budget, counting scalar/container
  entries, strings, bytes and object keys. Each individual value remains capped
  at 16 KiB.
- Each regex has a 256 KiB compiled NFA size limit and a 256 KiB lazy DFA cache
  limit. These are regex-engine component limits rather than a measured process
  RSS guarantee. At most 16 regexes can inspect at most 16 KiB of input each per
  event (256 KiB combined regex input scanning budget).

Limits count disabled, expired and shadow rules too, because these rules are
still stored and compiled. Rejected snapshots leave publication and retention of
the last valid immutable snapshot to the caller. Stateful rate limiting remains
unsupported.

Run `cargo test --offline -p hydra-filter-engine` and
`cargo clippy --offline -p hydra-filter-engine --all-targets -- -D warnings`.
