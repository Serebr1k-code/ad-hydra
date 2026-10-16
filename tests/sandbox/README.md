# Real sandbox integration

The Cargo-discovered, opt-in kernel integration test lives in
`apps/hydra-core/src/infrastructure/sandbox/mod.rs` as
`actual_linux_network_limits_and_cancellation`.

See [sandbox integration](../../docs/sandbox-integration.md) for exact host delegation,
fixed TCP echo target, helper mode and execution prerequisites. It is ignored by default
because this repository cannot manufacture operator cgroup delegation. Missing prerequisites
must be reported as unavailable; never replace this test with unsandboxed execution.
