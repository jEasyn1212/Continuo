# References and third-party code

Architecture research reference: [CC Switch](https://github.com/farion1231/cc-switch), MIT License, copyright (c) 2025 Jason Young. Reviewed commit: `2db86e94da13365caae55bb08d09295e31500d21`.

This initial implementation is independently written; no CC Switch source files were copied. If subsequent changes copy or substantially adapt source, retain the upstream copyright and MIT license, identify the affected files and originating commit here, and preserve their notices.

Agent protocol/configuration references: [Claude Code CLI](https://code.claude.com/docs/en/cli-reference), Codex CLI help, Hermes CLI parser, [MCP specification](https://modelcontextprotocol.io/specification/2025-11-25). No private profiles or configurations are included.

Dependencies are tracked in Cargo and npm manifests and lockfiles. Their licenses remain with their authors; review notices when packaging a distributable application. This project uses RustCrypto's ChaCha20-Poly1305 rather than implementing cryptographic primitives.

The MCP check client is independently written against the MCP 2025-11-25 lifecycle and stdio transport specification. No SDK or reference-server source was copied. Unix process-group cleanup uses the existing `libc` crate as a direct dependency (MIT OR Apache-2.0); upstream notices remain in the Cargo dependency distribution.

Native registration contract tests use the `toml` parser (MIT OR Apache-2.0) as a dev-only dependency, so generated literal arguments are checked by a real parser.
