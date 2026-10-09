# Contributing

Continuo is an early prototype. Please discuss changes to product scope, supported agents or synchronization semantics before implementing them.

Build the core with `cargo test --workspace`. Build the renderer with `npm ci && npm run build` in `apps/desktop`. Check the Tauri manifest separately. Keep meaningful tests for concurrent updates, synchronization recovery, encryption and protocol boundaries.

Use temporary vaults and local bare Git repositories. Never attach credentials, private configurations, local databases, real transcripts or encryption keys to issues, fixtures or pull requests.

Interfaces share the operation catalog and service layer. Integrations implement adapters with explicit capabilities. Preserve source attribution and licenses when borrowing code. Read [AGENTS.md](AGENTS.md) for workspace rules and [docs/decisions.md](docs/decisions.md) for scope.
