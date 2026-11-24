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

Desktop setup restores the local service from its own saved opt-in, then starts
previously connected external servers in the background. SQLite stores a nullable
`startup_digest` of the exact confirmed external configuration, not resolved
credentials. Existing rows migrate without consent. The following low-level client
example connects once; only the desktop lifecycle records startup intent.

```rust,ignore
let servers = mcp::McpServers::new(db.clone());
servers.seed_presets()?;
// Explicit user settings action, never an Agent tool:
servers.write_secret(&settings, "mcp.glossmod_http.api_key", &token)?;
servers.set_enabled("glossmod_http", true)?;
let config = servers.get("glossmod_http")?;
// The desktop Start action authorizes the displayed configuration.
// Pass false here because this example uses HTTP.
let consent = mcp::ConnectionConsent::after_user_confirmation(&config, false)?;
let runtime = mcp::McpRuntime::default(); // HTTP does not need a bundled launcher.
let client = mcp::McpClient::connect(servers, &settings, &config.id, consent, &runtime, &cancel).await?;
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

The presets below appear in Settings → 工具.

- Mod客栈: Streamable HTTP at `https://www.glossmod.com/api/v1/mcp`, Bearer token
  reference `mcp.glossmod_http.api_key`. With no saved key the public tools are available to guests.
- 3DM: Streamable HTTP at `https://mod.3dmgame.com/mcp`. Tool discovery works without an API key. Some queries require authentication.
  If configured, `mcp.glossmod_3dm.api_key` supplies the raw `Authorization` header.

| Mod source | Bundled uvx arguments | Settings → 工具 → 凭据 |
| --- | --- | --- |
| mod.io | `modio-mcp@latest` | Dashboard HTTPS `/v1` API path in `MODIO_API_BASE_URL` and matching `MODIO_API_KEY` |
| CurseForge | `curseforge-mcp@latest` | `CURSEFORGE_API_KEY` from CurseForge for Studios |
| GameBanana | `gamebanana-mcp@latest` | Anonymous |
| Nexus Mods | `--upgrade --from glossmod-nexusmods-mcp nexusmods-mcp` | Anonymous public lookup, optional `NEXUSMODS_API_KEY` for account-authorized operations |
| [Thunderstore](https://pypi.org/project/thunderstore-mcp/) | `--default-index https://pypi.org/simple thunderstore-mcp@latest` | Anonymous |

The five stock uvx launchers above pass their provider-specific `*_DOWNLOAD_DIR`
to the child process, set to `downloads/incoming` in the active storage directory.
Custom launch commands and arguments do not receive this override.

The stock Nexus launcher also gives uvx `--isolated --find-links` with the
packaged `nexus/` wheel directory. The directory supplies `1.0.0+amm.1` for
file records whose GraphQL `uri` is a storage key. The regular index remains
available for newer releases. A safe REST `file_name` or legacy archive name
is preferred. Otherwise the downloader uses the detected extension and file
UID to create the local name before requesting a mirror.

The wheel and its source/test patch live in `scripts/vendor/`. To reproduce
it, check out `GlossMod/nexusmods-mcp` at the `sourceCommit` in
`scripts/mcp-runtime.lock.json` with LF line endings and apply
`nexus-archive-filenames.patch` using `git apply`. In the upstream
`pyproject.toml`, set the project version to `1.0.0+amm.1` and the build
requirement to `hatchling==1.33.0`. Run `uv build --wheel` and compare the
wheel SHA-256 with the runtime lock. The local wheel can be retired once the
upstream index distributes the fix.

Schema v9 migrates the stock 3DM stdio preset to the remote endpoint and revokes the old launch consent. Reconnect from Settings to use the new transport.

- [Playwright MCP](https://github.com/microsoft/playwright-mcp) uses the bundled
  Node.js and Playwright CLI on Windows, macOS and Linux. The preset command is
  `amm-playwright --browser=chromium --headless --sandbox --isolated --no-webmcp` with
  `--file-paths=absolute` and `--output-dir` set to `downloads/Playwright` under
  the active manager storage directory, not the system Downloads folder.
  Starting the tool prepares the matching Chromium build in the user's Playwright
  cache before the MCP handshake. The first download needs network access.
  Setup allows up to ten minutes, respects download proxy settings and can be retried from Settings.
  Later connections reuse that build. Sessions use a temporary browser profile.
  Upgrading the stock preset or moving storage changes the output argument and
  expires its saved startup consent. Click Start once in Settings → 工具 to authorize the new command.
  Custom launch commands and browser selections are preserved.
- [MarkItDown MCP](https://github.com/microsoft/markitdown/tree/main/packages/markitdown-mcp):
  `uvx --prerelease allow markitdown-mcp@latest`. This preset accepts prereleases for MarkItDown and its dependencies.
  Converts documents to Markdown via stdio, without an API key. Its `file:`
  URIs can read files available to the OS user, outside the manager's game roots.

Click Start on a service in Settings to connect immediately. A stdio command runs with the current OS user’s permissions. Bare `npx`, `npx.cmd`, `npx.exe`, `uvx` and `uvx.exe` (case-insensitive) use
application-owned launchers. Their child environments select npmmirror
(`npm_config_registry`) or the Tsinghua PyPI mirror (`UV_DEFAULT_INDEX`).
Explicit CLI registry options take precedence. MarkItDown inherits the bundled uvx default.
Thunderstore resolves `@latest` on official PyPI because the bundled mirror did
not serve the package when the preset was added. Schema v25 updates the stock
preview command pinned to 0.1.0 and expires its auto-start consent; use Start to
approve the new command. Custom launch configurations are preserved.
Other commands, including absolute paths, keep their own executable and package configuration.
Package-backed MCPs and their Python interpreters are fetched by their launchers.
Schema v4 revokes old auto-start consent for affected bare aliases, including custom connections,
while preserving commands, arguments and credentials.
Schema v11 updates the six stock package-backed presets to resolve current versions at launch.
After migration, click Start once to restore automatic reconnection for each changed command.
User-supplied commands and package arguments are preserved.
Schema v6 removes the redundant index argument from the stock MarkItDown preset.
Users who had enabled automatic startup confirm the updated command once. Custom commands,
arguments and package indexes are preserved.

Schema v3 switches only an unmodified built-in Windows Playwright preset to Edge.
If it was set to start automatically, start it manually once to confirm the new
command. Custom browser arguments and other edited configurations are preserved.

For a first `uvx` launch the app allows up to ten minutes for dependencies and
MCP initialization, with up to four concurrent downloads and a 120-second uv
HTTP timeout in the child environment. A failed start can be retried from Settings. The app does not log child stderr.

## Trust Boundary

- Generic external tools use `Effect::ExternalWrite`, even when annotations claim
  read-only behavior. `AgentRuntime` is responsible for approval before execute.
  Its existing explicit `FullTrust` policy bypasses external approval; this
  module does not claim an additional unforgeable approval capability.
- `ConnectionConsent` is single-use, bound to exact config, non-serializable and
  intended for trusted UI/IPC code only. Do not expose it, configuration mutation,
  process startup or `write_secret` as model-callable tools. Enabling alone never
  authorizes spawning. Desktop startup may issue fresh single-use consent when
  the current config matches a receipt saved after successful manual connection
  and registration. Stop, edit, disable or remove revokes the receipt. A failed
  restore leaves it available for the next launch, until explicitly cancelled.
  Agent registry registration never starts a process.
- The official Mod 客栈 `get_work_file_download` has a desktop-only adapter. The approval
  names the work and file IDs and covers signing and transferring that one archive.
  `Effect::UncatalogedDownload` requires approval unless the desktop Agent uses Full Trust. Only the adapter consumes the
  signed URL, using the bounded public HTTPS downloader with no MCP authorization header.
  The model and download catalog receive the resulting local artifact.
- **stdio is NOT sandboxed.** Its command and descendants have the user's OS
  filesystem/network permissions. The user authorizes the selected connection with Start. A malicious
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
  for the exact HTTP and environment credential values resolved at startup, including after credential
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

Connection initialization: HTTP 20 seconds, direct `uvx` launchers 600 seconds,
other stdio launchers 120 seconds. Each discovery page has a 30-second response deadline. Ordinary tool calls run until the provider replies, the connection fails, or the user cancels.
Cancellation disconnects failed requests, sends best-effort SDK cancellation,
and kills the direct stdio child. Timeout/cancellation cannot guarantee that an
external side effect did not happen; never blindly retry it.

Discovery follows server pagination and rejects malformed schemas, duplicates and
non-advancing cursors. The desktop limits allocation for a connection according to
available memory and concurrent work. Ordinary tool calls can run until completion
or cancellation. A single stdio line or HTTP SSE event must fit the connection's
frame allowance. The rmcp SDK limits HTTP JSON responses to 16 MiB and error bodies
to 64 KiB. Queues and concurrent HTTP requests provide transport backpressure.

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
Default tests use no production endpoint credentials or actual uvx invocation.
Opt-in native smoke from the repository root: `node scripts/test-rust.mjs --lib live_markitdown_preset_initializes_and_lists_conversion_tool -- --ignored`
(from `src-tauri`). It prepares the preset, initializes it and lists tools, but
never converts a document or sends one to the server.

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
