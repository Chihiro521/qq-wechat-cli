# Repository Guidelines

## Project Structure & Architecture

Target Windows x64 and Node.js 24. Development also requires Rust stable and MSVC C++ Build Tools.

- `src/`: TypeScript CLI, engine communication, output formatting, and streaming exports.
- `native/src/`: Rust decryption, snapshots, WAL handling, platform adapters, SQLite indexing, and queries. Keep QQ and WeChat parsing in their respective modules.
- `test/`: Node integration tests and synthetic fixtures; `native/tests/fixtures/`: cryptographic reference vectors. Rust unit tests live alongside implementation modules.
- `scripts/`: packaging and installation checks; `research/`: development references and benchmarks; `docs/`: validation records.
- `dist/`, `native/target/`, and `native/engine.exe` are generated outputs.

The CLI and Rust exchange versioned NDJSON. Coordinate protocol changes across both languages; reserve stdout for protocol frames and stderr for progress.

## Build, Test, and Development Commands

- `npm ci`: install locked development dependencies.
- `npm run build`: compile both languages, copy the core executable, and generate dependency notices.
- `npm run check`: check TypeScript without emitting files.
- `npm run test:rust`: run native unit tests.
- `npm test`: run Node integration tests; rebuild first after source changes.
- `cargo fmt --manifest-path native/Cargo.toml --check`: verify Rust formatting.
- `node dist/cli.js qq status`: run locally; substitute `wechat` for WeChat commands.
- `npm pack --pack-destination artifacts`: create the installation archive; verify it with `node scripts/smoke-package.mjs artifacts/qq-wechat-cli-0.1.0.tgz`.

## Coding Style & Naming

Use two-space indentation, single quotes, semicolons, and explicit types in TypeScript. Follow strict NodeNext ESM conventions, including `.js` import extensions. Use `camelCase` for TypeScript functions and variables, `PascalCase` for types, and Rust `snake_case` conventions with `rustfmt`.

## Testing Guidelines

Use `node:test` and `node:assert/strict` in `test/*.test.mjs`; use Rust `#[test]` modules. Add behavior-focused regressions for parsing, WAL commits/resets, rollback, account isolation, and streaming exports. Keep fixtures synthetic. There is no enforced coverage percentage.

## Commit & Pull Request Guidelines

Use concise imperative subjects, such as `fix: preserve committed WAL frames` or `feat: initialize unified QQ and WeChat CLI`. Use `codex/` branches for agent work. PRs should explain changes, link issues, list validation commands, and document schema or protocol changes. Maintain packaging allowlists and attribution notices.

## Local Data & Performance

Use `--data-dir` for isolated validation. Keep real databases, account material, and exports in ignored directories. Remove temporary datasets and build caches after large experiments while retaining deliverables. Report initialization, refresh, and cached-query timings separately; compare equivalent workloads. Preserve the five-second cumulative client-pause limit and recovery watchdog.
