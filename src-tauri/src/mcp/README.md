# Outbound MCP Clients

## Dependency Verification

On 2026-11-17, `cargo info rmcp@3.10.0 --verbose` downloaded and inspected
the official Model Context Protocol Rust SDK. `cargo info rmcp@3.0.0` also
confirmed that 3.x exists. This module pins **rmcp 3.10.0**, not an invented
version. Official sources inspected:

- https://github.com/modelcontextprotocol/rust-sdk
- https://docs.rs/rmcp/3.10.0/rmcp/
- https://crates.io/crates/rmcp/3.10.0

rmcp requires Rust **1.88**. The project currently declares Rust 1.85; the
tested toolchain is 1.94. Integration must raise the declared MSRV or explicitly
select an older SDK. This change does not alter that declaration. rmcp uses
reqwest 0.13, so `reqwest-mcp` aliases that version without replacing the
application's reqwest 0.12 dependency.

## Integration

The parent agent owns `lib.rs`, commands and runtime integration. Add
`pub mod mcp;` there. No schema migration is required.

```rust,ignore
let servers = mcp::McpServers::new(db.clone());
servers.seed_presets()?;
// Explicit user settings action, never an Agent tool:
servers.write_secret(&settings, "mcp.glossmod_http.api_key", &token)?;
servers.set_enabled("glossmod_http", true)?;
let config = servers.get("glossmod_http")?;
// Display config (command, arguments, endpoint, secret refs) and require the user
// to confirm this connection. For stdio also require acknowledgement of the
// unsandboxed OS-user process boundary; pass true only after acknowledgement.
let consent = mcp::ConnectionConsent::after_user_confirmation(&config, false)?;
let client = mcp::McpClient::connect(servers, &settings, &config.id, consent, &cancel).await?;
let registered = client.register_tools(&agent_registry, &cancel).await?;
// Keep the client. On disabling/removing/editing settings or application shutdown:
client.disconnect();
// On shutdown, also reap the direct stdio child:
client.disconnect_and_wait().await?;
```

`McpServers` exposes create/get/list/update/remove/set_enabled. Preset seeding
is insert-only and all presets are disabled by default. It never launches a
process or overwrites an existing configuration. Configuration has no plaintext
env/header credential fields. `env_json` stores env-name -> encrypted-Store
references only. `write_secret` delegates to existing Settings/SecretStore;
secret values enter neither SQLite nor config serialization/debug output.

Presets configure third-party services; they do not bundle the runtimes:

- Mod客栈: Streamable HTTP at `https://www.glossmod.com/api/v1/mcp`, Bearer token
  reference `mcp.glossmod_http.api_key`.
- 3DM: `uvx glossmod-mcp`, environment `GLOSSMOD_API_KEY` resolved from reference
  `mcp.glossmod_3dm.api_key` at launch.
- [Playwright MCP](https://github.com/microsoft/playwright-mcp):
  `npx --yes @playwright/mcp@0.0.85 --headless --isolated --no-webmcp`
  (`npx.cmd` on Windows). Requires Node.js 20+, npm, and a supported browser
  such as Chrome for navigation. Uses a separate temporary browser profile;
  it does not attach to an existing signed-in browser. No API key is needed.
- [MarkItDown MCP](https://github.com/microsoft/markitdown/tree/main/packages/markitdown-mcp):
  `uvx --with azure-ai-contentunderstanding==1.2.0b3 markitdown-mcp==0.0.1a8`,
  with uv and Python 3.10+. The explicit dependency pin lets uv resolve the
  prerelease required by MarkItDown without allowing all prerelease packages.
  Converts documents to Markdown via stdio, without an API key. Its `file:`
  URIs can read files available to the OS user, outside the manager's game roots.

Enable and connect from Settings. For stdio, verify the displayed command and
acknowledge its OS permissions before connecting. The launcher must be on the
desktop application's `PATH`; edit the command to an absolute path when needed.
The first connection may download packages and exceed the 20-second handshake
timeout. MarkItDown can also download Python and document conversion dependencies.
To warm a cold cache, run the trusted launcher command with `--help` appended
in a terminal as the same user, then retry the connection. These package versions
are fixed in new preset entries; existing entries stay editable and are not
auto-updated.

## Trust Boundary

- All external tools are `Effect::ExternalWrite`, even when annotations claim
  read-only behavior. `AgentRuntime` is responsible for approval before execute.
  Its existing explicit `FullTrust` policy bypasses external approval; this
  module does not claim an additional unforgeable approval capability.
- `ConnectionConsent` is single-use, bound to exact config, non-serializable and
  intended for trusted UI/IPC code only. Do not expose it, configuration mutation,
  process startup or `write_secret` as model-callable tools. Enabling alone never
  authorizes spawning. Agent registry registration never starts a process.
- **stdio is NOT sandboxed.** Its command and descendants have the user's OS
  filesystem/network permissions. User acknowledgement is mandatory. A malicious
  server can directly delete or alter local files outside FileEngine; per-tool
  approval cannot contain that process. Use only trusted executables/packages,
  or keep stdio disabled. A real OS sandbox/container is required for stronger
  containment; none is implemented or claimed here. `uvx` and `npx` may download
  and run third-party package code as part of the confirmed startup.
- The stdio child gets a minimal environment and only explicitly configured secrets.
  It receives no inherited API keys, loader variables or stderr log forwarding.
  Cancellation/disconnect kills the direct child, not a guaranteed hostile
  descendant process tree. Disconnect does not roll back external effects.
- Each connection retains a private, non-Debug/non-Serialize credential filter
  for the exact Bearer/env values resolved at startup, including after credential
  rotation. Before results reach Agent/history, it redacts strings, object keys,
  structured content, metadata and canonical JSON-escaped text as `[REDACTED]`.
  Both successful and `isError` results are filtered. Remote RPC/transport and
  credential-provider error details are suppressed. Tool descriptions are
  filtered; credential-bearing names/schemas are rejected rather than changing
  routing or a schema contract. Result bounds apply before and after filtering.
  This is exact-match filtering, not protection against arbitrary encodings,
  fragmented/transformed credentials or an unsandboxed process exfiltrating them.
- **Integration logging follow-up:** rmcp itself logs some raw peer information
  and notifications before application result filtering. The shared desktop
  subscriber must disable `rmcp` events (for example `info,rmcp=off`) or add a
  credential-aware logging filter. This module does not change shared logging
  initialization and does not claim SDK diagnostic logs are already sanitized.
- Remote servers get no advertised roots, filesystem/exec/sampling/elicitation
  capability, no local tool callback, no automatic resource/link fetching.
  Tool results are untrusted data, never local commands or file operations.
- HTTP redirects are disabled; remote endpoints require HTTPS (HTTP allowed
  only for loopback). No credentials/query/fragment in URLs. No automatic
  expired-session replay of potentially state-changing requests.
- Tools use `mcp__<server-prefix>__<tool-prefix>_<sha256-prefix>` names,
  with ten-byte readable prefixes and 128 hash bits of `(server,NUL,tool)`.
  Names fit the OpenAI 64-character function-name limit as well as registry
  limits. Exact original names are retained in the bridge;
  registry collision detection fails rather than replacing existing tools.
  `register_tools` delegates to atomic `ToolRegistry::register_batch`: name
  conflicts or the registry's 256-tool cap leave no partial additions.
- Disable/edit/remove is checked before each request; explicitly disconnect
  immediately on settings changes to stop an already-running request as well.
  Desktop `McpConnections` integration revokes registered names on disconnect;
  clients also reject stale config before subsequent requests.

## Bounds And Cancellation

Connection handshake: 20 seconds. Listing/call: 30 seconds including pagination.
Cancellation disconnects failed requests, sends best-effort SDK cancellation,
and kills the direct stdio child. Timeout/cancellation cannot guarantee that an
external side effect did not happen; never blindly retry it.

Limits: 128 tools, 8 pages, 2 MiB total listing, 64 KiB schema, 16 KiB description,
256 KiB arguments, 1 MiB returned result. stdio lines and HTTP SSE events are
bounded at 2 MiB before parsing. rmcp's built-in HTTP JSON collector has a
16 MiB pre-parse cap (64 KiB error bodies); the stricter result cap is enforced
after parsing. SDK queues/concurrent HTTP requests are bounded.

## Tests

`tests.rs` uses an ephemeral loopback JSON-RPC HTTP mock and `mock_stdio.py`
(Python 3 test fixture only). It tests Bearer injection, stdio handshake and
encrypted-Store env references, defaults/CRUD, stale confirmation rejection,
ExternalWrite despite read-only annotations, namespace collision cases,
duplicate names/cursors, oversized results, timeout, cancellation, revocation,
direct stdio child exit/reaping after disconnect, atomic registration failure,
HTTP/stdio credential echo filtering (including quoted environment values),
key rotation, `isError`/RPC errors, filtered history/tool-call persistence,
provider errors and post-redaction expansion limits.
No production endpoint credentials or actual uvx invocation are used.

Verification after parent integration on 2026-11-17:

- `cargo test --lib --offline mcp::`: 14 passed, 0 failed (176 filtered).
- `cargo clippy --lib --no-default-features --offline -- -D warnings`: passed.
- Independent path-import harness Clippy including MCP tests: passed.
- Earlier `cargo test --lib --offline --quiet`: 139 passed, 1 failed in the unrelated
  `storage::tests::guard_blocks_outside_paths` (`storage.rs:136`); no storage
  code was changed by this module's implementation.

The temporary verification harness imports `mcp/mod.rs` by path and re-exports
the application's existing db/settings/agent interfaces. It does not modify
repository integration files.
