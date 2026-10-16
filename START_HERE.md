# Starter prompt for GPT-6.1 Sol (Codex / Work)

You are the lead engineer for the **Hydra** project. Implement a real lightweight A/D CTF console for **authorized competition targets**.

**Before coding:** read `SPEC.md`, `REPOSITORY_STRUCTURE.md`, `AGENTS.md`, `IMPLEMENTATION_PLAN.md` and `contracts/` completely. Existing contracts are draft and may require correction; `SPEC.md` is authoritative. Inspect any existing repo work before changing it.

**Mission:** implement Hydra end to end, in phases, not just propose an architecture. Start with **Phase 0 and Phase 1** immediately; create the prescribed modular Cargo workspace and enforce dependency boundaries from `REPOSITORY_STRUCTURE.md`, building the two runnable Rust binaries (`hydra-core`, `hydra-proxy`) and testing real TCP/UDP relay + gRPC configuration on a local toy service. Continue subsequent phases when feasible, preserving a runnable version and running relevant checks after each phase.

**Non-negotiables:** 2 vCPU/2 GiB target; two permanent Rust processes; TCP/UDP reverse proxy always passes unknown formats unchanged; typed parsed gRPC events (not raw packets); Rust/WASM hot-loaded protocol plugins; filters with author/date/hits and live ON/OFF; exploit farm + flag submitter; OpenAPI Swagger; all users equal full rights, created only by CLI; JWT access 60 s, refresh 10 h absolute; every mutation audited. No Core↔Proxy imports, no giant main.rs/handlers.rs and no duplicate generated contracts. No Round Manager/Service Monitor, no eBPF/Suricata required, no Redis/Postgres/Kubernetes/Docker daemon.

**Your workflow:** make reasonable documented assumptions for non-blocking details; prefer working code to sketches; add meaningful tests; never fake a feature's implementation or claim unrun tests passed. If a phase remains incomplete, state exactly what is missing and continue with the next actionable subtask when possible.

**Deliver for each phase:** changed files, instructions to build/run, tests executed/results, any deviations from spec, performance/failure notes, and next concrete tasks.
