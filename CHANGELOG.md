# Changelog

## 0.1.0

- Introduce `qq-wechat-cli qq ...` and `qq-wechat-cli wechat ...` using a TypeScript CLI and bundled Rust core.
- Add independent Classic QQ 9.7.25.29417 and WeChat database decryption, Windows snapshots, WAL validation, and a persistent SQLite search index.
- Support sessions, history, search, statistics, contacts, members, favorites, unread checkpoints, summary material, streaming exports, configuration import, and cache cleanup.
- Ship a Windows x64 package for Node.js 24, with cryptography, SQLite, and zstd included in the core executable.

Cached queries are fast after initialization. First indexing and changed-source refreshes can take substantial time on large databases; see `docs/validation.md` for measurements and comparison limits. WeChat key discovery depends on loaded client databases; explicit legacy configuration import is supported.
