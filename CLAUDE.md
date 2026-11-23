# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Agent Mod Manager (v0.1.0, not release-ready): a desktop game-mod manager whose core feature is an LLM agent that plans and applies file operations, including for registered games. Tauri v2 + Rust backend, React 19 + TypeScript frontend, single SQLite database.

`docs/implementation-status.md` is the running inventory of what is implemented, tested, and still missing. Read it before non-trivial work and update it when you finish a slice.

Data flow: React pages → `src/lib/api.ts::api()` → Tauri `invoke` → `src-tauri/src/commands.rs` → domain modules. The agent's turns live in the same Rust process and share the same `FileEngine`.

## Commands

```sh
yarn install --frozen-lockfile
yarn tauri dev                 # full desktop app (Rust + Vite)
yarn dev                       # frontend only, port 1420, native commands unavailable
yarn build                     # tsc -b && vite build (produces dist/ for Tauri bundles)
yarn typecheck                 # tsc --noEmit

node scripts/test-rust.mjs --lib
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
```

Run Rust tests through `scripts/test-rust.mjs`. Each run gets a private directory under `src-tauri/target/test-tmp/` on Windows or `/tmp` on Unix, cleaned after Cargo exits. Never use the user profile root as `TEMP`, `TMP` or `TMPDIR`.

Run a single Rust test by name filter: `node scripts/test-rust.mjs --lib transaction::tests::`. Single Playwright test: `yarn test:ui -g "trash restore"`; viewport projects are `desktop-1280`, `desktop-1440`, `narrow-390`.

CI (`.github/workflows/verify.yml`) runs `yarn build`,  `cargo fmt --check`, `node scripts/test-rust.mjs --lib`, `cargo clippy --all-targets -D warnings`, then unsigned bundles for Windows/macOS/Linux.

## Hard invariants

These are load-bearing; breaking them defeats the safety design the rest of the code assumes.

- **Never bypass `transaction::FileEngine` for generic Agent file operations or write game or manager files directly with `std::fs`.** Build a `FilePlan` (fingerprinted `before`/`source`, per-entry `impact`), obtain approval according to policy, then `apply`. Every transaction write runs under the process-wide `WRITE_LOCK` with a durable journal, overwrite backups, and reverse rollback; startup `recover_interrupted()` handles crashes, and unrecovered state blocks new plans.
- **The desktop Agent's native `read_dir`, `read_file`, `extract_preview`, `plan_file_ops`, and `apply_file_ops` can use paths outside the app data root and registered game roots.** They use the desktop process's OS permissions and still protect credentials (including `private/`, `.env`, and `.env.*`), enforce path safety checks, and follow the write approval policy. `extract_preview` only lists archive entries; `preview_fomod` stays limited to managed downloads and staging. Keep existing root restrictions for inbound MCP clients and other desktop operations. Game registration is still explicit where required; discovery is not write authorization.
- **`write_text` is exclusive to the native desktop Agent's `plan_file_ops`.** Each operation supplies `dst` and the complete UTF-8 `content` to create or overwrite a file. Planning does not write target files; `apply_file_ops` follows the normal approval policy. A plan containing `write_text` requires review in `AutoApprove` even inside a registered game root. Inbound MCP and other generic planners must reject this operation.
- **Only desktop transaction-history rollback and the local Agent may undo external file transactions.** `rollback_transaction` uses the recorded scope and still validates journal ownership and paths; inbound MCP cannot undo these transactions, even with the transaction ID.
- **Permanent deletion exists only in `trash::purge_trash`**, reachable solely from the desktop IPC (`purge_trash` with an exact `PURGE_TRASH` confirmation). It must never be added to the agent tool registry or to the external MCP allowlist in `mcp_server.rs`.
- **Game card actions are desktop IPC only.** Register `update_game`, `remove_game`, `open_game_location`, and `launch_game` only in `lib.rs`'s `generate_handler!`. The Agent tool registry and the `mcp_server.rs` `NATIVE_TOOLS` allowlist must exclude all four commands.
- **Approval is a policy matrix, not a boolean.** `agent/policy.rs` maps `Effect` × `PermissionMode`; its test asserts the exact table. Writes outside the app data root and registered game roots are not `GameWrite` and require review in `AutoApprove`. New tools must declare the correct `Effect`, and `prepare()` must return the preview/plan that `execute()` then commits unchanged.
- **`main.rs --mcp-stdio` only proxies** to an already-running desktop server. Never open a second database or a second writer from the stdio path.
- **`SourceKeys` is deliberately not `Serialize`/`Debug`**. `ResolvedMod` is internal and not serializable, and its `Debug` redacts `download_url` (short-lived CDN signatures). Desktop, Agent and MCP use `sources::resolutions` for scoped one-use IDs and a separate redacted wire DTO; do not accept raw URLs or caller-supplied provider metadata in catalog downloads.
- **Uncataloged direct links use the separate `download_web_mod` Agent tool**. It requires approval unless the Agent is in Full Trust, accepts public archive URLs only, does not assign catalog provenance, and is not exported by inbound MCP.
- **Never invent a provider API, endpoint, or install path.** Unsupported states are reported explicitly (see `docs/mod-sources.md`).

## Backend architecture (`src-tauri/src/`)

- `lib.rs` — all `#[tauri::command]`s are registered in `generate_handler!`; adding a command means editing this list. `run()` is `#[cfg(not(test))]`, so desktop wiring isn't compiled during `cargo test`.
- `app.rs` — `AppServices::initialize`: logging, DB, publication recovery, transaction recovery, legacy built-in adapter retirement, plugin/skill/install reconciliation. Tauri state is `AppServices`, `AgentRuntime`, `LocalMcpServer`.
- `transaction.rs` — `FileEngine`, `FilePlan`, journal, backups, snapshots, cross-volume handling. It sits on `safe_fs.rs` (+ `safe_fs_windows*.rs`), which resolves each path component relative to a held directory handle to resist path-replacement races; `filesystem.rs` is the read-only listing/reading surface over the engine. `db.rs` holds one `Arc<Mutex<Connection>>` (WAL, FULL sync); keep guards scoped and never hold one across an `.await`. Commands wrap synchronous work in `commands.rs::blocking` (`spawn_blocking`).
- `agent/` — `runtime.rs` (turn loop, SSE client, cancellation, durable event log with seq replay, context compression), `tools.rs` (`AgentTool` trait + `ToolRegistry`), `policy.rs`, `approval.rs`, plus one module per tool family (`install_tools`, `source_tools`, `diagnostic_tools`, `plugin_tools`, `web_tools`, …). Tool families self-register in `AgentRuntime::new`; registering also requires the tool to appear in `ToolRegistry::builtin`'s name list or its own `register()`.
- `install.rs` / `archive.rs` / `imports.rs` — adapter-driven install previews, archive safety checks (traversal, links, size and ratio limits), and desktop-only local archive import (never exposed as an agent tool).
- `games.rs` / `game_roots.rs` / `plugins.rs` / `skills.rs` — discovery and declarative adapter/Skill packages. The desktop starts with an empty game adapter registry. External packages supply `adapter.json` and `SKILL.md` through the draft/install workflow or a release-pinned desktop catalog install. Direct catalog install and installed-plugin removal are desktop IPC actions, not Agent/MCP tools. Adapter fixtures in `src-tauri/tests/fixtures/game-adapters/` are used only in tests.
- `sources/` — read-only `ModSource` providers (Modrinth, Thunderstore, key-gated CurseForge, Nexus ID lookup, 3DM API v3); failures are isolated per provider. Offline contract tests replay `sources/fixtures/*.json`.
- `download.rs` / `download_tasks.rs` / `artifacts.rs` — resumable, hash-verified, quarantined downloads that stay inert and cannot install themselves.
- `mcp/` + `mcp_connections.rs` — outbound third-party MCP clients (stdio is an acknowledged unsandboxed OS process; disabled by default). `mcp_server.rs` — inbound loopback server with an encrypted-Store bearer token and an explicit `NATIVE_TOOLS` allowlist.
- `settings.rs` / `encrypted_store.rs` — settings are public; API keys and bearer tokens live in encrypted Tauri Store, with SQLite storing only references. See `docs/credential-storage.md` for the runtime master key.

## Frontend (`src/`)

- `App.tsx` — shell, sidebar navigation, lazy-loaded pages (`src/pages/*`), `PlanDialog` mounted globally, and a shortcut to pending background Agent approvals. The active chat renders `AgentConfirmation` inline.
- `lib/api.ts` — the only IPC entry point; throws a clear error when `backendAvailable` is false. `useBackendQuery` wraps TanStack Query. The browser preview must never fabricate data or simulate success — a UI test asserts this.
- `lib/agent-store.ts` — stream store keyed by conversation; events merge by `seq`; replay cursors advance only from durable `replay_agent_events` fetches, never from live events.
- `lib/store.ts` — zustand UI state (view, theme, toasts, pending plan review).
- UI is built from shadcn/ui (new-york style, Radix, `lucide-react`). Primitives live in `src/components/ui/` (generated by `npx shadcn@latest add`, then `from "cn"` rewritten to `@/lib/utils`; do not hand-edit beyond the local variants noted below). `src/components/ui.tsx` holds the app wrappers: `Page`, `PageHeader`, `IconButton` (Tooltip + aria-label), `EmptyState`, `ErrorState`, `Badge` (legacy `tone` → variant), `PathText`, `Modal` (Dialog; closes on Escape/button, not outside click), `bytes`, `date`. Pages compose these; there is no page-level stylesheet.
- `styles.css` is only the Tailwind v4 entry plus theme tokens (`:root` and `:root[data-theme="dark"]`, oklch). Colors are semantic tokens (`bg-background`, `text-muted-foreground`, `text-success`, `text-warning`); never raw palette colors or manual `dark:` color overrides. The app toggles `data-theme` on `<html>`, and the `dark:` variant is keyed on it. Local variants added to generated components: `Badge` success/warning/danger, `Alert` warning/success, `Sidebar` always visible (desktop app, no mobile sheet), `Spinner` decorative (`aria-hidden`). Motion lives in `src/motion.css` (keyframe tokens, imported by `styles.css`) and `src/lib/motion.ts` (`stagger`, `useDidChange`). Components opt in with `motion-safe:animate-*`, and the global rules sit inside `prefers-reduced-motion: no-preference`, so reduced motion turns every effect off. Animate opacity and transform only; do not move an element while it fades in, because Playwright clicks the position it measured.
- Selects stay native (`NativeSelect`) on purpose: Playwright uses `selectOption`/`locator('option')`. Keep `<details>/<summary>` native where specs assert keyboard disclosure. Notices go through `useUI().notify` (sonner). `src/lib/i18n.ts` exists but is only partially adopted; UI copy is hard-coded Chinese.
- Game cards (`LibraryPage`) offer launch, open folder, Mod folder, edit and remove. The UI only sends a game id (plus `root`/`mods`), never a path; the backend resolves and validates it.

## Testing

Rust tests are colocated (`#[cfg(test)] mod tests`) and in sibling `*_tests.rs` modules; they drive real `tempfile` directories and mock HTTP, never live APIs. The exceptions are `#[ignore]`d: live Modrinth/Thunderstore/CDN tests (`agent/live_modrinth_tests.rs`, `sources/live_provider_tests.rs`), a Fabric diagnostic test that needs logs from `scripts/run-fabric-resolver-smoke.mjs`, and a crash-recovery worker spawned by its parent test in `transaction_recovery_tests.rs`. Run them explicitly with `-- --ignored <name>`. Packaged-app smoke scripts for each OS live in `scripts/` and are documented in `README.md`. Playwright tests never touch the backend: `tests/ui/mock-tauri.ts` injects a fake IPC transport via `page.addInitScript`, and production code must not import it. All user-facing strings in Rust errors are Chinese; keep new ones consistent.
