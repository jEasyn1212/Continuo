# Working on Continuo

- This is a new independent product. Do not migrate personal configurations or modify reference projects.
- Current supported agents: `claude-code`, `codex`, `hermes`. Adding another requires an explicit product decision.
- Keep domain logic in `continuo-core`. CLI, MCP and desktop are interface adapters; route them through `Service::call` and the shared operation catalog.
- New agent integrations implement `AgentAdapter`, capability descriptors and contract tests. Do not scatter agent-specific branches into storage or interface code.
- No Continuo-hosted backend in v1. Synchronization uses storage configured by the user.
- Identity instructions, account authentication and enforced permissions are different concepts. Never claim prompt injection provides security isolation.
- Do not upload secrets, local paths, live SQLite files, existing runtime state or raw internal conversation state as portable configuration.
- Preserve concurrent versions, tombstones and causal history. Do not resolve conflicts using wall-clock timestamps or force pushes.
- Stdout of MCP must contain only newline-delimited JSON-RPC. Send diagnostics to stderr. Enforce interface authorization in the service layer.
- Run meaningful tests against temporary directories and a temporary bare Git repository. Never use real agent profiles or users' private sync repositories as fixtures.
- Validate with `cargo test --workspace` and `npm run build` in `apps/desktop`. Tauri is a separate Cargo manifest; check it separately after desktop changes.
- Retain source, commit and license information for borrowed code. Do not introduce license obligations silently.
- Never commit local databases, keys, credential files, generated binaries or build caches.
