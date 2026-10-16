# Hydra — coding-agent handoff

## Start
1. Upload/extract this directory into your coding agent's working repository.
2. Give GPT-6.1 Sol the text from `START_HERE.md` as its first message.
3. Keep `AGENTS.md` at repository root so Codex can use persistent guidance.
4. Read `REPOSITORY_STRUCTURE.md` as the **mandatory** layout/dependency contract and `SPEC.md` as authoritative product requirements. `contracts/` contains previous reviewed **drafts** to validate and evolve, not a deployed API.
5. Work through `IMPLEMENTATION_PLAN.md`, requiring actual binaries/tests after each milestone.

## Contents
- `SPEC.md` — consolidated requirements and acceptance criteria.
- `REPOSITORY_STRUCTURE.md` — **mandatory** complete file tree, crate boundaries, dependency directions, code ownership, runtime paths, contract generation, CI rules and scalability acceptance criteria.
- `AGENTS.md` — persistent engineering rules for agent.
- `IMPLEMENTATION_PLAN.md` — phases and tests.
- `START_HERE.md` — copy-paste prompt.
- `contracts/openapi.yaml` — OpenAPI 3.1 draft (39 paths / generated schemas).
- `contracts/hydra/proxy/v1/proxy.proto` — Protobuf3 gRPC draft.
- `contracts/API_DESIGN_v1.1.md` — legacy design notes (defer to `SPEC.md` on inconsistencies).
- `examples/*.yaml` — sample filter rules.

## Readiness
Requirements and initial draft contracts only; this package does NOT contain Hydra's executable implementation. Build and security verification are part of the agent's assignment.
