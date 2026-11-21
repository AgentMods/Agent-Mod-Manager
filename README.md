# Agent Mod Manager

Agent-driven game Mod management with Tauri v2, React 19, TypeScript and Rust.
The full implementation goal is active. This repository is not release-ready;
see [implementation status](docs/implementation-status.md) for verified work
and remaining requirements.

The current development build includes Skill management, declarative plugin
draft/install controls, Brave web search,
and a reviewed plugin-publication workflow. Checks on November 19, 2026 include
594/594 passing Rust library tests, a successful `yarn build`, and 210/210
Playwright checks. The desktop local-archive install flow includes a bounded
FOMOD configuration preview. Players can search for and download Mods through
Agent Chat.
Unknown-game identification now has reviewed public searches, untrusted
candidates, independent user confirmation and a 24-hour evidence cache. It does
not infer an adapter or automatically install anything. See
[game identification](docs/game-identification.md) and the implementation status.

The 3DM source now uses a separate key-gated API v3 search/resolve adapter.
Only unique verified first-party resource URLs become download candidates;
cloud shares and ambiguous or unverifiable compatibility selections stay manual.
See [Mod source restrictions and verification](docs/mod-sources.md).

A current macOS arm64 `.app` (38.58 MiB) was rebuilt successfully for local
testing. CI now also performs an isolated launch smoke that verifies SQLite
initialization and the six embedded adapters. An earlier `.dmg` (13.90 MiB) had an ad hoc signature, not Apple
Developer ID or notarization. `hdiutil verify` reported `VALID`, and a
temporary copy from that DMG booted with an isolated home directory. Gatekeeper
rejected the build, so this is not distribution acceptance. An earlier build
passed a local inert Minecraft archive import and separate install approval
smoke test. Ubuntu 24.04
ARM64 `.deb` installation and AppImage extraction-mode launch both passed the
native WebView smoke in Docker. Normal desktop and FUSE launch are not covered.
Authenticated remote publication and Windows-native verification remain pending.
No real mod was downloaded or published during these checks.

## Development

Use Node.js 22 or newer, **Yarn 1.22.22**, Rust 1.88 or newer, and the native
Tauri build prerequisites for the operating system.

```sh
yarn install --frozen-lockfile
yarn tauri dev
```

The packaged macOS launch smoke can be run locally on a clean desktop session
with an absolute `.app` path:

```sh
node scripts/run-macos-package-smoke.mjs \
  "$PWD/src-tauri/target/release/bundle/macos/Agent Mod Manager.app" \
  "$PWD/output/macos-package-smoke"
```

Configure an OpenAI-compatible API URL, a model name and optional API key in
Settings. Credentials are AES-256-GCM encrypted in Tauri Store; the runtime
master key is kept in a private `.env` file. SQLite stores references only.
See [credential storage](docs/credential-storage.md) for migration, configuration
and the security tradeoff. No model is silently selected and no live API
credentials are included in the project.

For frontend-only work:

```sh
yarn dev
```

The browser preview is intentionally disconnected from native commands. It
does not invent a game library, perform file changes or simulate successful
Agent operations. Automated browser tests inject a separate test-only IPC
transport; production code does not import that transport.

## Verification

```sh
yarn build
yarn playwright install chromium
yarn test:ui
cargo test --manifest-path src-tauri/Cargo.toml --lib
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --no-default-features -- -D warnings
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
```

The CI workflow tests and builds unsigned Windows NSIS/MSI, macOS DMG and Linux
AppImage/deb artifacts. The Ubuntu job installs the Debian package, runs the
extended native smoke against both packages, and saves screenshots and logs.
Separate Windows jobs install each bundle in a fresh runner and drive the
installed app's real WebView2 through the same extended Agent scenario. They
save installer logs and screenshots. Hosted results and platform acceptance
status are tracked in `docs/implementation-status.md`.

For Windows, use a disposable VM with no Agent Mod Manager installation or profile,
PowerShell 7, Node 24, WebView2 and `yarn install --frozen-lockfile`. Use an
elevated PowerShell for MSI. Run one installer per clean VM or snapshot:

```powershell
./scripts/run-windows-package-smoke.ps1 -Kind msi -Installer 'C:\packages\Agent Mod Manager_0.1.0_x64_en-US.msi' `
  -EvidenceDir 'output\windows-package-smoke\msi' -DisposableMachine
```

For NSIS, use `-Kind nsis` and pass the `-setup.exe`. The wrapper runs the real
installer and does not uninstall it or remove its test profile. It verifies
game registration, Fabric diagnostics, import through the Windows file chooser,
live streaming from a local test model, reviewed install and trash operations,
cancellation, and archive replay and restoration after restart. It checks the
resulting SQLite journal in read-only mode. The scenario uses a temporary game
directory and an inert fixture archive and requires no provider credentials.

To run the same extended test on Linux, install `tauri-driver` with
`cargo install tauri-driver --locked --version 2.1.0`. The wrapper requires
Node 22+, the distribution's matching `webkit2gtk-driver`, `dbus-run-session`,
`xvfb-run` with `xauth`, `xdotool`, `curl`, `sqlite3` and CJK fonts. Run it as a
non-root user, with ports 4444 and 4445 free; don't run two instances at once:

```sh
bash scripts/run-linux-package-smoke.sh /usr/bin/agent-mod-manager output/linux-package-smoke/deb
```

For an AppImage, pass its path and a separate evidence directory. The wrapper
uses extraction mode, not FUSE. It uses temporary `HOME` and XDG directories,
a private Xvfb display and a separate session bus. The profiles are removed on
exit; only test logs and screenshots are retained in the evidence directory.

To run the smaller diagnostic and trash smoke directly, use a disposable non-root user profile with
an isolated `HOME` and XDG directories, a graphical session (or Xvfb and D-Bus),
Node 22 or newer, and the distro's matching `WebKitWebDriver`. Start
`tauri-driver --port 4444` in that session, then run:

```sh
node scripts/smoke-desktop-webview.mjs /usr/bin/agent-mod-manager /tmp/amm-native-smoke.png
```

For an AppImage, pass its absolute path instead of the installed binary. In a
container without FUSE, start the driver with `APPIMAGE_EXTRACT_AND_RUN=1` and
record this as extraction-mode verification only. The script uses synthetic
Minecraft files and never calls a provider or launches the game.

Set `AMM_LINUX_AGENT_SMOKE=1` on the Node command for the extended run. This
requires `xdotool`, a private Xvfb display (not the user's desktop), and a fresh
absolute `XDG_DATA_HOME` shared by the driver and script. It selects an inert
JAR in the real GTK chooser, installs it via the Agent after approval, and
checks streaming diagnosis, cancellation, recovery of a trashed test note,
and history replay after relaunch. The only model endpoint is a deterministic
loopback SSE fixture; no credentials or internet access are needed.

## File Safety

In desktop Agent Chat, `read_dir`, `read_file`, `extract_preview`,
`plan_file_ops`, and `apply_file_ops` can work outside registered game roots
and the manager data root, subject to the desktop process's OS permissions
and the app's file safety checks. `extract_preview` only lists archive entries;
it does not extract or install anything. `preview_fomod` still requires an
archive in managed downloads or staging. Credential storage and `.env` files
remain protected, and game discovery still does not grant write authorization.
Inbound MCP clients keep their existing filesystem restrictions.

The desktop Agent's native `plan_file_ops` accepts `write_text` with a `dst`
path and the full UTF-8 `content` for a new file or a complete overwrite. The
combined text content in one plan may not exceed 1 MiB. Planning does not
write the file; applying the plan follows the normal approval policy and still
requires review in AutoApprove mode, even for registered game directories.
Inbound MCP and other generic file planners cannot use `write_text`.

File changes are planned and fingerprinted before approval, then committed with
a durable journal and overwrite backups. Writes to external directories are not
game writes and still require review in AutoApprove mode. Conflicting files are
not silently replaced during restoration. Rollback of an external transaction
is supported by the desktop transaction-history command and the local Agent,
not by inbound MCP. Failed or interrupted transactions are recovered before
new changes are allowed.

Agent removal tools move content into manager trash. Only a separate,
explicitly confirmed desktop action can permanently purge selected trash
payloads. That action is not exported to Agent or MCP tools.

Third-party stdio MCP servers run with the user's OS privileges, **not** inside
the file transaction sandbox. They are disabled by default and require explicit
trust acknowledgement. Do not enable an untrusted process. Descriptor-level
defenses against concurrent filesystem path replacement and stronger stdio
containment remain acceptance requirements.

Desktop installers include Node.js with npm/npx and Astral uv/uvx. MCP connections using
a bare `npx` or `uvx` command can install packages without system Node or uv.
By default, npx uses `https://registry.npmmirror.com`, and uvx uses
`https://pypi.tuna.tsinghua.edu.cn/simple`. To override a mirror, put `--registry=<url>`
before the npm package name or `--default-index <url>` before the Python tool name.
These defaults apply only to the child process. Explicit executable paths keep their own
configuration. A first start can still need network access for packages and Python.
The Playwright connection also requires its selected browser. Upgrading users must
confirm previously started bare launchers once to use the packaged runtime.

Settings remembers each MCP connection after its first successful start. On the
next app launch, it reconnects in the background without opening Settings.
Stop or edit a connection to cancel auto-start. After upgrading from a version
without this feature, start each desired connection once to opt in.

## Local MCP Server

The local server is opt-in from Settings, binds a random port on numeric
loopback, and requires a fresh random Bearer token. The token is held in the
encrypted Tauri Store under reference `mcp.local_server.token`.
The server starts again on subsequent launches until you stop it in Settings.
Each start rotates the token and binds a new endpoint. Desktop status
returns the endpoint and reference, not the token.

External clients share the desktop's tool registry, journal, permission policy
and approval coordinator. Clients cannot approve their own writes. Only an
explicit native tool allowlist is exported; external namespaces, purge and
connection/process configuration controls are excluded.

Clients requiring stdio can run the executable with `--mcp-stdio`, supplying
`AMM_MCP_ENDPOINT` and `AMM_MCP_TOKEN` through their secure environment setup.
Never put tokens in command-line arguments, committed JSON or chat. Stdio mode
only proxies to an already running desktop server; it does not open a second
database or instantiate a second writer.

## Adapters And Skills

Six built-in declarative game adapters live in `plugins/games/`. Adapters and
Skills never execute code. External packages are authored as inert drafts,
validated, reviewed and installed through two-file transactions. A schema pass
does not prove a package's compatibility or instruction trustworthiness.
Publication uses a fixed-origin, explicitly reviewed workflow; authenticated
live uploads have not yet been acceptance-tested.

Application data contains `manager.db`, `downloads/`, `staging/`, `backups/`,
`trash/`, `plugins/`, `skills/` and `logs/`. Backups and failed staging files are
retained rather than automatically removed.
