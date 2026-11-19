# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Agent Mod Manager (v0.1.0, not release-ready): a desktop game-mod manager whose core feature is an LLM agent that plans and applies file operations on registered game directories. Tauri v2 + Rust backend, React 19 + TypeScript frontend, single SQLite database.

`docs/implementation-status.md` is the running inventory of what is implemented, tested, and still missing. Read it before non-trivial work and update it when you finish a slice.

Data flow: React pages → `src/lib/api.ts::api()` → Tauri `invoke` → `src-tauri/src/commands.rs` → domain modules. The agent's turns live in the same Rust process and share the same `FileEngine`.

## Commands

```sh
yarn install --frozen-lockfile
yarn tauri dev                 # full desktop app (Rust + Vite)
yarn dev                       # frontend only, port 1420, native commands unavailable
yarn build                     # tsc -b && vite build (produces dist/ for Tauri bundles)
yarn typecheck                 # tsc --noEmit
yarn test:ui                   # Playwright; starts its own Vite on 127.0.0.1:1430

cargo test --manifest-path src-tauri/Cargo.toml --lib
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
```

Run a single Rust test by name filter: `cargo test --manifest-path src-tauri/Cargo.toml --lib transaction::tests::`. Single Playwright test: `yarn test:ui -g "trash restore"`; viewport projects are `desktop-1280`, `desktop-1440`, `narrow-390`.

CI (`.github/workflows/verify.yml`) runs `yarn build`, `yarn test:ui`, `cargo fmt --check`, `cargo test --lib`, `cargo clippy --all-targets -D warnings`, then unsigned bundles for Windows/macOS/Linux.

## Hard invariants

These are load-bearing; breaking them defeats the safety design the rest of the code assumes.

- **Never write game or manager files directly with `std::fs`.** Everything goes through `transaction::FileEngine`: build a `FilePlan` (fingered `before`/`source`, per-entry `impact`), get approval, then `apply`. Every write runs under the process-wide `WRITE_LOCK` with a durable journal, overwrite backups, and reverse rollback; startup `recover_interrupted()` handles crashes, and unrecovered state blocks new plans.
- **Paths outside a registered root are unreachable.** `AppPaths::guard` allows only the app data root plus registered game roots, rejects the `private/` directory and `.env*`, and rejects symlink-replaced roots. Discovery results never grant write permission; registration is explicit.
- **Permanent deletion exists only in `trash::purge_trash`**, reachable solely from the desktop IPC (`purge_trash` with an exact `PURGE_TRASH` confirmation). It must never be added to the agent tool registry or to the external MCP allowlist in `mcp_server.rs`.
- **Approval is a policy matrix, not a boolean.** `agent/policy.rs` maps `Effect` × `PermissionMode`; its test asserts the exact table. New tools must declare the correct `Effect`, and `prepare()` must return the preview/plan that `execute()` then commits unchanged.
- **`main.rs --mcp-stdio` only proxies** to an already-running desktop server. Never open a second database or a second writer from the stdio path.
- **`SourceKeys` is deliberately not `Serialize`/`Debug`**. `ResolvedMod` is internal and not serializable, and its `Debug` redacts `download_url` (short-lived CDN signatures). Desktop, Agent and MCP use `sources::resolutions` for scoped one-use IDs and a separate redacted wire DTO; do not accept raw URLs or caller-supplied provider metadata in catalog downloads.
- **Uncataloged direct links use the separate `download_web_mod` Agent tool**. It always requires approval, accepts public archive URLs only, does not assign catalog provenance, and is not exported by inbound MCP.
- **Never invent a provider API, endpoint, or install path.** Unsupported states are reported explicitly (see `docs/mod-sources.md`).

## Backend architecture (`src-tauri/src/`)

- `lib.rs` — all `#[tauri::command]`s are registered in `generate_handler!`; adding a command means editing this list. `run()` is `#[cfg(not(test))]`, so desktop wiring isn't compiled during `cargo test`.
- `app.rs` — `AppServices::initialize`: logging, DB, publication recovery, transaction recovery, built-in plugin seeding, plugin/skill/install reconciliation. Tauri state is `AppServices`, `AgentRuntime`, `LocalMcpServer`.
- `transaction.rs` — `FileEngine`, `FilePlan`, journal, backups, snapshots, cross-volume handling. `db.rs` holds one `Arc<Mutex<Connection>>` (WAL, FULL sync); keep guards scoped and never hold one across an `.await`. Commands wrap synchronous work in `commands.rs::blocking` (`spawn_blocking`).
- `agent/` — `runtime.rs` (turn loop, SSE client, cancellation, durable event log with seq replay, context compression), `tools.rs` (`AgentTool` trait + `ToolRegistry`, 256-tool cap), `policy.rs`, `approval.rs`, plus one module per tool family (`install_tools`, `source_tools`, `diagnostic_tools`, `plugin_tools`, `web_tools`, …). Tool families self-register in `AgentRuntime::new`; registering also requires the tool to appear in `ToolRegistry::builtin`'s name list or its own `register()`.
- `install.rs` / `archive.rs` / `imports.rs` — adapter-driven install previews, archive safety checks (traversal, links, size and ratio limits), and desktop-only local archive import (never exposed as an agent tool).
- `games.rs` / `game_roots.rs` / `plugins.rs` / `skills.rs` — discovery and declarative adapter/Skill packages. Built-in adapters are `include_str!`-embedded from `plugins/games/<slug>/{adapter.json,SKILL.md}` and listed in `BUILTIN_PACKAGES`; a new built-in game needs both files plus an array entry.
- `sources/` — read-only `ModSource` providers (Modrinth, Thunderstore, key-gated CurseForge, Nexus ID lookup, 3DM API v3); failures are isolated per provider. Offline contract tests replay `sources/fixtures/*.json`.
- `download.rs` / `download_tasks.rs` / `artifacts.rs` — resumable, hash-verified, quarantined downloads that stay inert and cannot install themselves.
- `mcp/` + `mcp_connections.rs` — outbound third-party MCP clients (stdio is an acknowledged unsandboxed OS process; disabled by default). `mcp_server.rs` — inbound loopback server with an encrypted-Store bearer token and an explicit `NATIVE_TOOLS` allowlist.
- `settings.rs` / `encrypted_store.rs` — settings are public; API keys and bearer tokens live in encrypted Tauri Store, with SQLite storing only references. See `docs/credential-storage.md` for the runtime master key.

## Frontend (`src/`)

- `App.tsx` — shell, sidebar navigation, lazy-loaded pages (`src/pages/*`), `PlanDialog` and `ApprovalDialog` mounted globally.
- `lib/api.ts` — the only IPC entry point; throws a clear error when `backendAvailable` is false. `useBackendQuery` wraps TanStack Query. The browser preview must never fabricate data or simulate success — a UI test asserts this.
- `lib/agent-store.ts` — stream store keyed by conversation; events merge by `seq`; replay cursors advance only from durable `replay_agent_events` fetches, never from live events.
- `lib/store.ts` — zustand UI state (view, theme, toasts, pending plan review).
- `styles.css` — Tailwind v4 is imported, but the bulk of styling is hand-written there using `:root` custom properties and `[data-theme="dark"]`. `src/lib/i18n.ts` exists but is only partially adopted; UI copy is hard-coded Chinese.

## Testing

Rust tests are colocated (`#[cfg(test)] mod tests`) and in sibling `*_tests.rs` modules; they drive real `tempfile` directories and mock HTTP, never live APIs. Playwright tests never touch the backend: `tests/ui/mock-tauri.ts` injects a fake IPC transport via `page.addInitScript`, and production code must not import it. All user-facing strings in Rust errors are Chinese; keep new ones consistent.
