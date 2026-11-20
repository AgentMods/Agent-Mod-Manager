# Agent Mod Manager implementation status

Updated: 2026-11-19. The full project goal remains active. This is an
implementation and verification snapshot, not a release-completion claim.

After removal of Mod Discovery, all 171 remaining Playwright cases passed on
Windows across the three configured viewports. `yarn typecheck` and the Vite
production build also passed. The provider model picker scrolls within its
panel in both themes with 81 models, including a long model ID. Strict Clippy
and Rust formatting passed in the preceding Rust verification.

Rust library tests: 608 passed, 4 failed, 5 ignored, with `TEMP`/`TMP` outside
`AppData` (game identity rejects scan roots under the default `AppData` temp
directory). The four existing Windows failures are
`diagnostics::tests::noncanonical_database_roots_are_rejected`,
`diagnostics::tests::records_cannot_authorize_relative_outside_manager_or_other_game_paths`,
`imports::tests::filenames_are_portable_and_match_the_install_contract`, and
`paths::tests::existing_regular_file_is_not_an_ancestor`.
`src-tauri/build.rs` embeds `windows-app-manifest.xml` (Common Controls v6) into
Windows binaries, including test executables, to avoid `STATUS_ENTRYPOINT_NOT_FOUND`.

The encrypted credential round-trip test now passes on Windows after flushing
the credential file through a writable handle. An isolated Windows WebView2
smoke fetched 81 models from a local fixture, selected the last row, saved
settings with an encrypted test credential, and authenticated again after
restarting the app.

The macOS ARM64 debug `.app` and `.dmg` built successfully. The app launched
from the mounted read-only DMG under an isolated profile, passed SQLite
`quick_check`, and seeded six plugins. The DMG SHA-256 is
`3f56aca902edccf90491eb10d6edeef6a3c97dc629b2fc82958c49785de92dfd`.
Native distribution acceptance remains tracked below.

The product name is `Agent Mod Manager` and its Tauri identifier is
`com.glossmod.amm`. A rebuilt macOS ARM64 debug app confirmed both bundle
metadata fields and initialized a healthy database with six plugins in the
new app-data directory. Existing profiles require a separate migration. The
Windows and Linux smoke scripts use the new identity for future package runs.

## Implemented

- Yarn 1 lockfile, React 19/Vite/Tailwind dependency scaffold and Tauri v2 config.
- SQLite WAL schema, FULL synchronous journal writes, application storage layout.
- Desktop storage relocation is available under Settings → Appearance and storage.
  The native folder picker selects the parent for a new `Agent Mod Manager`
  folder. A confirmed restart copies and verifies the database and managed files,
  relocates stored paths, and switches the active directory after verification.
  The original data remains as a backup. A rejected migration leaves the previous
  directory active and displays the error in settings. Encrypted credentials and
  the location record remain in the default private directory. Windows path
  prefixes are hidden in the settings display.
- File plans with per-file impact, source/destination SHA-256 preconditions,
  approval state, process-wide write lock, durable per-entry intent/completion,
  overwrite backups, reverse rollback, interrupted-transaction recovery.
- Pending recovery blocks new file plans and execution.
- File transaction recovery records object identity before each exclusive rename.
  New files and directories are staged before promotion; rollback no longer
  treats a byte-identical competitor at the destination as its own output.
  Cross-volume recovery uses the identified retained original. Conflicting
  committed rollback requests are rejected before mutation; ambiguous pending
  recovery remains blocked. Legacy journal history remains readable, but rows
  without identity evidence cannot be automatically reversed.
- Core transaction file opens, creation, and renames use no-follow directory
  descriptors and exclusive renames on macOS/Linux. Windows now traverses
  validated local paths by handle, rejects reparse points, and uses a no-replace
  rename on the identity-checked source handle. Rollback refuses link-redirection
  of its recorded paths. See
  [filesystem operation safety](filesystem-safety.md) for the precise scope.
- Trash moves/restores use the transaction engine. Manifest and DB metadata are
  recorded before mutation. Restored visibility is derived from committed
  restore transactions, so rollback restores visibility as well as files.
- Only `trash::purge_trash` permanently removes files. The desktop-only IPC
  requires explicit `PURGE_TRASH` confirmation; no Agent/MCP tool exposes it.
  Durable purge tombstones preserve uninstall state and prevent invalid rollback.
- Cross-volume moves copy and verify before retaining the source on its own
  volume. Transaction-owned files live in `<game>/.amm-retained/<txn>/` or
  manager backups. They are omitted from snapshot inventories. Storage is not
  reclaimed automatically. Cross-volume tests inject EXDEV; real Windows volume
  and power-loss tests remain pending.
- Snapshot creation, checksum validation, restoration with retained additions,
  and rollback of snapshot restoration. Snapshot planning rejects redirected
  registration paths, uses identity-checked directory listings and file reads,
  and checks that the proposed transaction agrees with the inventory and manifest.
- ZIP/7z/tar.gz safety checks: path traversal, links/devices, 4 GiB entry limit,
  exact total 100:1 ratio, actual stream byte limits, exclusive staging targets.
  Preflight and extraction share one open artifact, verify its hash through
  that handle, and compare entry lists. On macOS/Linux and Windows, the extractor
  holds a staging directory handle for entry writes. The install preview
  requires exact staged source paths and hashes; failed staging is retained
  for inspection or later trashing.
- Six embedded declarative adapters and Skills: Minecraft Java, Skyrim SE,
  Fallout 4, Stardew Valley, Cyberpunk 2077, Baldur's Gate 3. Validation covers
  schema identity, versions, relative paths, references and Skill frontmatter.
  Built-in package and Skill reads tolerate the regular backup files retained by
  reseeding and still verify the current declarations.
- The adapter manifest provides optional `coverUrl` for the library and package
  details. The six built-in manifests point to official game artwork. Missing or
  unavailable covers fall back to the game placeholder. Existing manifests can
  omit the field. `loadOrder.kind` accepts `none` for games without a dedicated
  load-order format.
- Players can start a prefilled Agent chat from the game library and package
  center or request a new or updated adapter in chat. The Agent can find
  registered packages, read verified package contents, and use the existing
  draft, validation and installation transactions. Local updates keep the slug.
  Built-in packages can be copied under a new slug. The desktop library applies
  a selected installed package to a registered game after checking its markers.
- Settings shows three embedded system Skills for adapter authoring, web search
  and Mod downloads, plus manually installed Skills in their own section.
  Game Skills are available from their package details. The Agent can read the
  active system Skills and the selected game's package Skill.
- Game discovery: bounded shallow scans, marker confidence with ambiguous
  candidates, structured Steam ACF and libraryfolders parsing, secondary Steam
  libraries, Epic/Legendary metadata, explicit canonical root registration,
  idempotent game state persistence and list/get commands. Discovery does not
  grant write access or execute launcher metadata.
- First-run onboarding: desktop-only bootstrap detection with persisted
  `onboarding_completed`, explicit scan-candidate registration, Agent settings
  setup with encrypted API-key storage, and a three-stage feature tour. The
  onboarding UI uses local React shadcn/ui-compatible Button/Card/Badge/
  Progress/Tabs/Alert/Dialog components, supports light/dark themes and a
  narrow 390px layout, and can be reopened from Settings. Browser preview keeps
  its existing disconnected behavior and never fabricates scan results.
  `get_bootstrap_state` reports `onboardingCompleted`, `isFirstRun` (not
  completed and no registered games) and `resumeOnboarding`. The first
  registration inside automatic onboarding records a private in-progress marker
  through `start_onboarding`, so quitting mid-way resumes the guide; existing
  users with registered games are not interrupted. Finishing or "退出引导"
  calls `set_onboarding_completed(true)`. The desktop WebView smoke script
  dismisses the guide before its library workflow. Existing pages keep their
  `src/components/ui.tsx` imports; Button, IconButton, Badge and ErrorState now
  render through the shadcn/ui components while `Modal` stays a native dialog.
- Onboarding and AI settings share an editable model table with provider import,
  manual add, drag and keyboard sorting, and removal. Display names are saved
  independently of request IDs and appear in the default model and chat menus.
  New models use BaseLLM metadata when available. The chat composer has
  an upward model menu and a stepped reasoning slider with a provider-default
  position. The Ultra position gives the slider an animated blue-violet glow
  and drifting stars, with a static version for reduced motion. Each turn
  validates its model and reasoning level against the configured list and uses
  the selected model's context budget. Selecting a
  model in the composer leaves the saved default intact. Legacy single-level
  settings load as one enabled option.
- Agent `ask_user` requests appear inline as a questionnaire: one step per
  question, with single-choice options, free text, back/next, and skip. The
  existing approval transport waits for the response and preserves skipped
  steps as `null`. Legacy single questions render as a one-step text card.
  Replay shows resolved/cancelled status; file-operation approval is unchanged.
- Explicit user-data root registrations, bound to game and adapter root ID.
  Stable canonical root checks reject replaced symlinks, including replacement
  with a different already-authorized game or manager directory.
- Adapter-driven install previews and installed-file manifests. JAR/ZIP payloads
  declared single-file stay intact; SMAPI directories retain their manifests;
  declared Data prefixes are stripped only after inspection. FOMOD archives now
  have a bounded read-only `ModuleConfig.xml` preview in the desktop
  local-archive install flow and the Agent. Folder directives expand descendants from the
  verified archive inventory into exact per-file mappings, including wrapped
  installers and empty destinations relative to the install root. The desktop
  flow preselects deterministic
  `SelectAll` and required single-option groups, show the current file mappings,
  and send those mappings to the existing installation review. Conditions or
  scripts, duplicate paths and other ambiguous rules require manual selection.
  Agent requests can opt into deterministic FOMOD planning using the adapter's
  `fomod` type. The backend derives mappings from the verified extraction used by
  the plan and returns the usual per-file approval preview. A Skyrim SE sandbox
  test covers wrapped folder expansion, approval, overwrite and rollback.
  The local mock SSE test verifies the complete approval plan, the matching
  committed journal, and the compact receipt returned to the model.
  Condition evaluation and priority resolution remain outside the automatic
  selection rules.
- Install/uninstall/restore/rollback state derives from durable file transactions.
  Modified/shared installed files are not silently uninstalled. Installs that
  replaced existing files require transaction rollback to restore originals.
  Download/staging-source plans retain mandatory review under AutoApprove,
  including newly created generic plans; partial/quarantined downloads cannot
  be deployed with ordinary file tools. FullTrust still respects path guards.
- A user-selected catalog version can replace an installed version of the same
  source/project in one reviewed transaction. It can back up same-path files,
  add new files, and trash files unique to the old version. Ownership and
  fingerprints are checked again before apply; shared, edited or unowned
  files are not overwritten. `superseded` state and replacement chains derive
  from the journal, including on restart. Desktop rollback has a separate
  preview/confirmation and rejects stale or incomplete rollback inputs. Trash
  entries from the replaced version require rollback rather than direct
  restore. See [reviewed version replacement](install-lifecycle.md#reviewed-version-replacement).
- App startup: logging, database initialization, recovery, embedded package seed,
  single-instance plugin. Rust IPC wrappers exist for current file/trash/snapshot
  and plugin operations; file-changing commands produce plans before apply.
- Rust Agent runtime: OpenAI-compatible chat completions SSE, incremental UTF-8
  and tool-call assembly, complete-stream gate before tool execution, bounded
  completion/frame sizes, tool loop, single active turn per conversation,
  cancellation during HTTP streaming and approvals, usage and progress events.
- Durable turn/event log with sequence-based replay, persisted assistant deltas,
  tool results and approval requests. Startup marks in-memory turns interrupted;
  subsequent turns complete interrupted tool-result groups before a model call.
  Failed turns display replayable diagnostics beside the affected chat message,
  including HTTP status or provider SSE error details. Model diagnostics are
  capped at 64 KiB and redact the configured API key before persistence.
- Explicit permission matrix: request_approval/auto_approve/full_trust, global,
  conversation and turn overrides; interactive approval responses verify their
  conversation. Read tools and game-target file plans have separate effects.
- Context compression summarizes old turns without removing original messages.
  The current task's complete tool groups stay intact. Budget accounting now
  includes serialized messages, the complete registered tool schema, chat/tool
  framing overhead, model-family-specific text heuristics, and a larger safety
  margin for unknown OpenAI-compatible models. Exact provider tokenizers remain
  unavailable, so the estimator intentionally favors early compression over an
  over-budget request.
- Conversation persistence, archive/restore, streaming updates and tool state;
  settings validation and encrypted Tauri Store credentials with SQLite references only.
- Source trait and aggregator: Modrinth/Thunderstore search/resolve, CurseForge
  key-gated integration, and Nexus scoped numeric/text search plus Mod-ID
  lookup/account-limited download links. Nexus text search uses a fixed
  GraphQL document, variable-bound game/term/pagination inputs, rejects partial
  GraphQL errors and foreign/duplicate results, and keeps the scoped `domain/id`
  identity in returned summaries.
  Missing/invalid source keys degrade independently. 3DM now has a key-gated,
  fixed-origin API v3 search/resolve adapter using the verified native field
  names. It validates game/project/resource bindings, supports exact resource
  selection and offset pagination, and limits automatic resolution to a unique
  known first-party direct resource. Cloud shares, ambiguous files and
  unverifiable compatibility filters require manual selection. Rounded size
  strings are not promoted to exact bytes and hashes are not invented. It does
  not launch/connect MCP or share Mod 客栈 credentials. See
  [source restrictions and evidence](mod-sources.md).
  Shared provider HTTP caching and rate limiting are implemented;
  Brave search opts out of caching raw third-party response bodies.
- Agent Chat provides the in-app Mod search and download workflow. The Mod
  Discovery page and its standalone download history and install dialog were
  removed. Mod source settings remain available.
- Modrinth releases can be browsed by game version and loader via the read-only
  desktop IPC, Agent and local MCP version list. Resolve the exact ID with the
  same filters before downloading. Thunderstore direct dependency declarations are now also
  exposed when resolving a package, with unavailable/removed declarations
  kept unknown rather than treated as satisfied. No update selection or
  loader compatibility is inferred. Thunderstore version browsing now validates
  the community/project identity, uses fixed-origin pagination, and marks missing
  game-version or loader declarations as limited rather than inferred.
- Modrinth and Thunderstore resolution reports expose the selected version's
  declared direct dependencies via desktop IPC, Agent and local MCP. Exact
  Modrinth version lookups work with or without a project ID, rejecting
  mismatched identity or loader/game-version constraints. Valid empty
  declarations are distinct from unknown metadata; unavailable Thunderstore
  declarations remain unknown. No dependency lookup downloads files,
  authorizes a write, or implies compatibility.
- Agent, local MCP and desktop IPC can resolve bounded Modrinth required
  dependency graphs using an exact root version and explicit Minecraft version
  and loader. Ambiguous project-only dependencies require a selection; user pins
  are revalidated without overriding exact declarations. Results expose unknown
  metadata, observed conflicts, cycles and truncation. No files are downloaded
  or installed by the graph tool. The separate desktop IPC
  `review_modrinth_dependency_graph` supports exact candidate selection and
  reports per-edge local records for a Minecraft Java target. For a selected required edge with a unique exact
  enabled install, it checks the managed files against the recorded manifest's
  sizes and SHA-256 hashes. It distinguishes changed, missing and unverified
  files from complete matches; the investigation is bounded and read-only. It
  uses an immutable download catalog row rather than a UI-provided root, with
  no local IDs, paths, or file results sent to the source.
  See [transitive metadata](mod-sources.md#transitive-dependency-metadata).
- Agent and local MCP can independently run read-only `verify_install_files`
  for one enabled managed install of any source. It scopes the record to the
  supplied game, requires stable journal state, and returns local file evidence
  without reading anything from a provider or writing a repair plan.
- Agent, local MCP and desktop IPC `inspect_install` now include a read-only
  reconciliation preview. It identifies shared-path blockers, replacement
  predecessors/successors and a candidate rollback transaction when the durable
  journal proves one exists. The recommendation is advisory only; execution
  still goes through the existing ownership checks, approval policy and rollback
  transaction.
- Modrinth download installation can compare those direct declarations with the
  target game's exact catalog-backed records. Desktop, Agent and local MCP share
  the same read-only check, deriving active records from the install journal.
  The report distinguishes exact versions, project-only matches, different
  versions, absent records and ambiguous or partial installations. It can omit
  the selected replaced record without modifying it. No loader state or current
  file integrity is inferred. See [dependency review](install-lifecycle.md#modrinth-dependency-review).
- Downloads: progress/cancellation, retries, strong ETag/Content-Range validation,
  resumable partials across manager instances, size and SHA-256/SHA-512/SHA-1/MD5 checks,
  URL/redirect/DNS checks and quarantine. Downloads stay inert and cannot install.
  Every supplied hash must match. Weak algorithms retain review/warnings and do
  not confer strong verification. Completed-artifact metadata persists without
  signed URLs; installation checks both recorded filename and SHA-256.
- Completed downloads have independent file IDs and immutable catalog rows;
  repeat downloads no longer rebind the previous file's provider IDs. New
  catalog-backed installs persist the exact source/project/version in the
  transaction preview and installed record. Conflicting installation hints are
  rejected. Game Details, Agent and MCP expose the recorded origin without
  guessing from old rows or treating local hashes as proof of authorship.
  See [install lifecycle](install-lifecycle.md#exact-download-origin).
- New catalog downloads in Desktop, Agent and local MCP accept only an expiring,
  scope-bound, one-use resolution ID. The same backend record supplies the
  reviewed filename, source IDs, byte count, hashes and actual download URL.
  Signed CDN URLs are not serialized into public resolution or approval payloads;
  old structured links are omitted from replay to the model, not erased from disk.
  Pinned CurseForge project/file mismatches fail closed. See
  [catalog download binding](mod-sources.md#catalog-download-binding).
- Desktop download commands emit progress events, allow cancellation and list
  completed artifacts. Agent download completion uses the same artifact catalog.
- The Agent's separate `download_web_mod` tool fills the uncataloged direct-link
  fallback: explicit approval in all permission modes, public HTTPS with
  independently validated redirects, 1 GiB cap and archive signature check.
  It retains rejected bytes and records valid bytes as unverified `web` artifacts
  without catalog identifiers. It never installs and is not exported by inbound
  MCP. Completed-download promotion uses checked exclusive renames on native
  desktop targets. See [reviewed public archive links](mod-sources.md#reviewed-public-archive-links).
- Adapter-aware, bounded, read-only diagnosis: declared logs and load-order files,
  numbered evidence, missing-file observations, loader/dependency heuristics,
  explicit unknowns and repair suggestions. No guessed fix is automatically run.
  Text diagnosis now supports BOM-tagged UTF-16, an explicit legacy log-codepage
  fallback, and Windows-1252 Bethesda plugin lists. BG3 checks complete LSX
  profile structures and identifies internal UUID inconsistencies without
  interpreting PAKs or inferring activation. Minecraft Java diagnosis recognizes
  supported Fabric startup logs, including versions, selected Mod IDs and
  versions, nested entries, and resolver failure with physical line numbers.
  Complete supported English and Simplified Chinese resolver exceptions
  distinguish missing required dependencies from mismatched versions,
  preserving the original requirement and reported versions. Unsupported or
  clipped details leave a generic resolver failure, not an inferred repair.
  Evidence stays scoped to the instance's registered `latest.log`, is shared by
  desktop, Agent, and local MCP, and is not proof of successful initialization
  or live activation. A separate read-only `mods/` scan compares bounded
  `fabric.mod.json` ID/version declarations with historical top-level log rows
  and distinguishes install record matches from unrecorded and ambiguous files.
  It never extracts or executes a JAR. See [diagnostic limits](diagnostics.md).
- Brave Web Search is registered as a read-only Agent tool. It uses the fixed
  HTTPS endpoint and the `sources.brave` Settings secret, limits queries to
  1..512 characters/75 words and counts to 1..10, rejects local/private paths
  and credential-bearing URLs, bounds result fields, and marks excerpts
  untrusted. It validates result URLs as public HTTP(S) and never fetches them.
  Missing credentials report `needs_configuration`; malformed or failed
  responses do not become fabricated successful results.
- Unknown-game identification has bounded local name previews, exact-query
  opt-in Brave searches, untrusted candidate selection and independent desktop
  registration consent. Search evidence is bound to the canonical root,
  metadata fingerprint, query and record digest, expires after 24 hours and
  is invalidated by a newer search or successful confirmation. Confirmation
  atomically preserves existing counts/metadata, rejects adapter replacement
  and pending recovery, and never infers an adapter, Steam ID or verified
  confidence. Agent searches require publication-level review in default and
  AutoApprove modes; root confirmation is not an Agent/MCP tool. See
  [game identification](game-identification.md) for privacy and path limits.
- rmcp 3.10 outbound stdio/Streamable HTTP clients, disabled-by-default presets,
  encrypted-Store secret references, explicit exact-config connection confirmation,
  namespaced tools, bounded results/cancellation, stale-config revocation and
  atomic registry registration. All external tool annotations remain untrusted.
  Playwright MCP and MarkItDown MCP join the Mod 客栈 and 3DM presets as
  opt-in configurations; they do not contribute credentials or bundle runtimes.
  A macOS stdio smoke test exercised Playwright initialization, tool discovery,
  and navigation. MarkItDown resolved its dependencies, but cold-cache wheel
  downloads timed out before its handshake. Neither preset has been verified
  through the native settings UI on Windows.
  Stdio is an explicitly acknowledged unsandboxed OS-user process, not contained
  by FileEngine; stronger cross-platform sandboxing remains required.
- External MCP connections use a compact list with a per-service credentials
  dialog for Bearer and stdio environment secrets. Each row shows 未启动,
  启动中, 运行中 (tool count) or 启动失败 with the reason, and has one start/stop
  button. Starting saves `enabled` itself, so the editor has no enable switch.
  HTTP starts in one click; stdio asks once, showing the command line, because
  the backend still requires the unsandboxed-process acknowledgement.
  Playwright covers both transports, masked inputs, keyboard dismissal and
  start/stop/failure states at 390, 1280, and 1440 px.
- An optional desktop-owned MCP server binds numeric loopback on a random port,
  requires a fresh Bearer token and exports a native-tool allowlist. The stdio
  executable proxies to that running server rather than opening another database
  or writer; external clients cannot approve their own writes.
- Agent tools now include scan/read, archive preview, file planning/apply,
  trash/restore, snapshot creation, installed Skill reads, ask_user, source search/
  resolve/download, install/list/uninstall/rollback and diagnosis/logs. The complete
  registry also includes the bounded web search and reviewed plugin-publication
  operations described above.
- Plugin publication has an exact package/file/metadata preview, digest-bound
  approval, a second explicit `PUBLISH_PLUGIN` confirmation, durable history and
  guarded remote upload/scan/publish phases. Unknown remote outcomes are not
  automatically retried. The desktop can verify upload completion, a completed
  safety scan, or a matching published work on the caller's own Mod 客栈 shelf
  without resending. Inconclusive states stay unchanged; authenticated live
  network publication has not been exercised.
- The frontend has eight lazy-loaded views (including game detail), light/dark
  themes, Skill management, plugin draft/validate/install/enable controls, and
  publication preview/history. These implemented surfaces are not evidence of
  cross-platform or release acceptance.

## Verification

Storage settings passed the Playwright regression with mocked desktop IPC at
both desktop widths and the narrow viewport, covering Windows display paths,
confirmation and cancellation, restart arguments, and retry after failure.
Frontend type-check, production build, and Rust all-target checks passed.
The Windows migration test
fixtures could not initialize storage under the sandbox. Automatic restart and
on-disk migration have not been verified end to end on a desktop build.

Live Modrinth acceptance on 2026-11-18 (macOS ARM64): the opt-in Agent smoke
used real public search, exact Sodium Fabric version `26nVNc41` resolution and
CDN bytes. The Agent waited for separate download and install approvals, kept
the temporary game's original file intact until apply, and deployed the
hash-verified JAR with exact download origin. A separately approved rollback
restored the fixture byte for byte and retained the download. The SSE model
was scripted and nothing was executed. See the repeatable test in
[Mod sources](mod-sources.md#live-download-and-deployment).

Live Thunderstore acceptance on 2026-11-19 (macOS ARM64): public search and
exact `HansenbansenTeam/HudToggle` `1.0.1` resolution led to a real 100,869-byte
ZIP download. With no source hash, the catalog kept its integrity status
unverified and review mandatory. Nothing was extracted or installed. The
resolver now rejects mismatched community/package/version fields or a download
path inconsistent with the result. A rerun exposed a false file-change error
when Tokio's pending write overlapped the next metadata check. The downloader
now waits for each chunk write before reporting progress or checking the next
chunk; the original path/length and final hash checks remain. An offline test
forces the buffered write to wait. Both opt-in flows passed after the fix.
See [Thunderstore exact download](mod-sources.md#thunderstore-exact-download).

Catalog download binding verification on 2026-11-19 (macOS ARM64): the Rust
suite passed 560 tests (3 opt-in tests/workers ignored by default), strict
all-target Clippy and formatting passed, and both opt-in real provider flows
passed with the new resolution IDs. The Modrinth flow used scripted SSE; the
Thunderstore flow exercised the desktop download service, not a native WebView.
The Yarn build and 192 mocked-IPC Playwright checks passed across all three
viewports. The regressions cover redaction, expiry/consumption, scope separation,
approval-time ID substitution, and rejection of forged MCP download descriptors.

Reviewed web-download verification on 2026-11-19 (macOS ARM64): the Rust suite
passed 572 tests (four opt-in tests/workers ignored by default). Scripted SSE
verified approval before network access in all three modes and rejected extra
arguments without network or file writes. Loopback transfers covered rejected
HTML, the byte cap, redirect validation and undisclosed CDN parameters. A real
HTTPS JAR download through the web transport preserved its exact bytes without
catalog provenance. The Modrinth install/rollback and Thunderstore inert
download both passed again against the shared downloader. Strict Clippy,
formatting, `yarn build`, and all 192 mocked-IPC browser checks passed. No live
model, native Windows transfer, or runtime game activation was exercised.

Native Fabric resolver smoke on 2026-11-19 (macOS ARM64, JDK 21): Minecraft
1.21.1 and Fabric Loader 0.16.14 generated native `logs/latest.log` files for
missing and mismatched required dependencies and a successful resolver
selection. Both failure cases stopped before the test Mod's entrypoint; the
baseline selected it, ran its entrypoint, and stopped at `eula=false`.
An opt-in replay passed all three logs through registered-instance diagnosis,
checking physical evidence lines, read-only behavior, and no repair plan.
The full Rust suite passed 575 tests (five opt-in tests/workers ignored by
default). Strict all-target Clippy, formatting, `yarn build`, and all 198
mocked-IPC Playwright cases across three viewports passed. The browser checks
show the detailed finding and source line without offering repair.
This was not a running server, client launch, or third-party Mod activation.
See [native smoke instructions](diagnostics.md#native-smoke).

Simplified Chinese Fabric verification on 2026-11-19 (macOS ARM64): the smoke
now runs missing and mismatched dependency cases under both `en-US` and
`zh-CN`. All five native logs passed diagnosis replay, including the exact
Chinese failing dependency line. Offline regressions also passed for explicit
GBK fallback, incompatible mixed-language blocks, single bounded intervals,
and wildcard ranges. Rust passed 578 tests (five opt-in tests/workers ignored
by default); strict Clippy, formatting, `yarn build`, and 15 diagnostic
Playwright cases across three viewports passed.

Nexus text-search contract verification on 2026-11-19: offline provider tests
exercise the fixed GraphQL document, JSON-bound game/term/offset/limit
variables, authentication headers, scoped project identities, pagination
status, empty/out-of-range pages, explicit unsupported compatibility filters,
GraphQL errors with partial data, and foreign/duplicate/malformed results.
The Rust suite passed 584 tests (five opt-in tests/workers ignored by default),
strict all-target Clippy, formatting, and `yarn build` passed. No API key or
live Nexus request was used; the UI smoke additionally covers the scoped
project identity and unsupported compatibility filter path. Authenticated
download permissions remain a
separate unverified gate.

Ubuntu 24.04 ARM64 native package verification on 2026-11-18: Rust 1.94.0,
Node 24.14.0, Yarn 1.22.22, and WebKitGTK 2.52.6. The locked release build
produced `Mod Manager_0.1.0_arm64.deb` (16.95 MiB) and
`Mod Manager_0.1.0_aarch64.AppImage` (88.16 MiB). `dpkg` installed the Debian
package as `mod-manager` and supplied `/usr/bin/agent-mod-manager`.

Both the installed executable and the AppImage passed
`scripts/smoke-desktop-webview.mjs` using Tauri driver 2.1.0 and the real
WebKitGTK driver in an isolated, non-root, network-disabled Docker Linux VM
session with Xvfb and D-Bus. The smoke registered and reopened a synthetic
Minecraft instance, verified matching and mismatched Fabric JAR versions at the
expected log lines, and verified that trash and restore previews made no
changes until approval and that the original bytes were restored. Each fresh
database passed `PRAGMA quick_check` and showed two committed transactions.
The Linux Rust library suite for that build passed 546/546. No game or
downloaded Mod was run.

Both packages also passed the extended Agent smoke in fresh profiles using
`scripts/run-linux-package-smoke.sh`. The test selected a metadata-only Fabric
JAR in the real GTK picker, approved the staging import, observed an unfinished
model delta, ran the real Fabric diagnosis tool, and approved installation and
an Agent-initiated trash move. Cancelling an incomplete tool call left its
target untouched. After restarting the app, the archived conversation replayed
in read-only mode and the original note was restored byte for byte. Each
database passed `quick_check` and contained four committed file transactions,
three complete turns, one cancelled turn and one archived conversation. The
test drove native IPC with a deterministic loopback model, not a live provider.
Logs and screenshots are under `deb-extended/` and `appimage-extended/` in the
local artifact directory below. The Ubuntu GitHub Actions job now runs this
wrapper for both packages; no hosted x86-64 result has been recorded yet.
Both Ubuntu ARM64 packages passed again after sharing the scenario with Windows.
Re-run logs: `deb-shared-webview/` and `appimage-shared-webview/`.

Separate disposable runner jobs for Windows NSIS and MSI now run the extended
Agent scenario. The PowerShell wrapper checks for a fresh installation and data
profile. Playwright drives the installed WebView2 over loopback CDP, and a
PID-scoped helper selects the metadata-only JAR through the native Windows file
chooser. The script verifies streamed diagnosis, staged import, install and
trash approvals, cancellation, archived history across a process restart and
byte-exact restoration. It checks SQLite read-only for four committed
transactions, three completed turns, one cancelled turn and one archived
conversation. JavaScript and PowerShell syntax checks and both embedded C# helper
compilations passed locally. NSIS/MSI, the native chooser and interactive Windows
11 acceptance still require execution on Windows.

A fresh Ubuntu ARM64 Debian release built on 2026-11-19 with Yarn 1.22.22 and
locked Rust dependencies passed the shared native test after extending the mock
model to handle history compression. The test observed seven tool-enabled chat
requests and two tool-free summary requests, then verified `quick_check`, four
committed transactions and the four persisted turns. Evidence is under
`output/linux-acceptance/artifacts/current-deb-agent.mMAa8c/smoke-rerun/`.
The rebuilt Debian SHA-256 is
`c5675a652a8cbcba60a8ea71ecbcc1e152e6bf3ce2c6c3c9421cf818659fcae7`.

The AppImage used `APPIMAGE_EXTRACT_AND_RUN=1` because the container had no
`/dev/fuse`; FUSE launch, a normal graphical desktop install, and other Linux
architectures remain unverified. The AppImage reported a GStreamer `appsink`
warning in the container; the smoke does not exercise media playback. Packages,
build logs and WebView screenshots are in the ignored local directory
`output/linux-acceptance/artifacts/`.

| Ubuntu ARM64 package | SHA-256 |
| --- | --- |
| Debian | `24647d1f1912e282a10eab2d175a8c8f9ee7cbf9bb2b75fe42c34f5212c33a43` |
| AppImage | `e9ef5cbbabee8b27f9e0f58930e2cc1fd8eff6b067d7df25abe2a0e5aa5398f0` |

Snapshot hardening passed 546/546 Rust library tests on macOS, including six
snapshot cases. Coverage includes a root replaced by a link to another
authorized root, a changed registration during restore setup, source contents
changed after inventory, replacement of an empty directory, and ambiguous
relative paths. Rust formatting and strict Clippy passed. These checks do not
establish a point-in-time inventory or native Windows acceptance.

The subprocess crash regression is enabled in the normal Rust library suite,
including the Windows job. On macOS ARM64 and Ubuntu ARM64 it verified that
SIGKILL after the backup rename and durable replacement intent, but before
replacement, is recoverable when reopening SQLite. The original bytes and
file identity come back, implicit parent directories are rolled back, and a
second recovery is a no-op. The follow-up suites reported 548 passed on macOS
and 547 on Ubuntu, with only the subprocess helper intentionally ignored on
each. This is not Windows execution, a desktop power-loss test, or proof of
whole-volume durability.

Run from `src-tauri`:

```sh
cargo test --lib
cargo check --no-default-features
cargo clippy --lib --no-default-features -- -D warnings
```

Windows handle traversal round (2026-11-18): the Windows primitives and native
regression tests passed isolated `x86_64-pc-windows-msvc` compilation and strict
Clippy. The macOS backend regression suite passed 541/541. The Windows tests
have not yet run on Windows. The full app's MSVC cross-check from macOS stopped
in `ring`'s native build because Windows C headers/tooling were unavailable;
no Windows installer was built or run.

The Fabric JAR comparison round (2026-11-18): `cargo test --lib` passed
541/541 on macOS. The new checks cover bounded JAR and manifest parsing,
duplicate manifest names, CRC and deflate boundaries, exact historical ID and
version comparison, same-game enabled install records, linked or replaced
objects, and read-only output through desktop diagnosis, Agent and local MCP.
`yarn build` and 189/189 Playwright cases passed at 1280, 1440 and 390 px;
Playwright mocks native IPC. Both `custom-imagegen` prototypes for the JAR
comparison were reviewed before the UI edit (`output/imagegen/fabric-jar-evidence-*`).
Strict all-target Clippy and Rust formatting passed. `yarn tauri build --debug
--bundles app` produced the macOS arm64 debug bundle. The Mac was locked, so
native WebView verification is pending. The inert fixture at
`output/fabric-jar-native-smoke/instance` contains one exact match and one
historical version difference; the fixture JARs contain metadata only.

The 522-test round added thirteen Rust checks for Fabric log grammar, nested
IDs, bounds, mixed sessions, scoped file reads, and numbered resolution failure
evidence through diagnosis and local MCP. `yarn build`, 183/183 Playwright cases
across 1280, 1440 and 390 px, `cargo check --no-default-features`, strict Clippy,
and formatting all passed. The browser uses mocked Tauri IPC. The new Fabric
diagnostics layout was implemented after reviewing both `custom-imagegen`
prototypes; see `output/imagegen/fabric-diagnosis-*` for images and prompts.
`yarn tauri build --debug --bundles app` also produced an arm64 macOS debug
bundle. Its direct launch under an isolated `HOME` initialized a fresh database
(`quick_check` returned `ok`) and seeded six plugins. The process was stopped
after the check. The Mac was locked, so the diagnostic screen was not exercised
in the native WebView. No game was registered or launched in this smoke.

The preceding 509-test round added twelve Rust checks for same-size edits, missing
and unsafe paths, exact-match eligibility, read limits, open-file identity,
and read-only Agent and local MCP routing. An isolated Windows MSVC harness
compiled the file-handle verifier and its tests, but did not execute them on
Windows. `yarn build` and 177/177 Playwright cases passed at 1280, 1440 and
390 px, including separate record/file states, stale replies, and light/dark
interaction. The browser suite uses a mocked Tauri bridge; this does not verify
the native WebView or a real game launch. The two file-evidence prototypes from
`custom-imagegen` were reviewed before implementation, with images and prompts
in `output/imagegen/modrinth-file-evidence-*`.

The previous 497-test round added five desktop graph review contracts for exact project pins,
game isolation, replacement exclusion, and rejection before a provider request
when the replacement is invalid or recovery is pending. Its install-preview
browser cases also cover keyboard use, explicit version selection, and the
absence of any write request. Its prototypes and prompts are in
`output/imagegen/modrinth-transitive-review-*`.

The preceding fourteen graph cases cover traversal, provider identity and
compatibility, deferred choices, conflict and limit reporting, read-only Agent
policy and MCP validation. A standalone production-code smoke
against the anonymous Modrinth API exercised an exact Iris-to-Sodium dependency,
an ambiguous project-only declaration, and an explicit selection. No archive
or local game was used. The previous nine transaction cases exercise file and directory
collisions, competing destinations after backup, interruption before/after
promotion and during rollback, retention collisions, and an older journal with
no ownership evidence. Existing replacement and plugin tests also confirm that a
byte-identical substitute backup and an out-of-order rollback are both refused
before mutation. The earlier archive/install regressions cover source and staging
links, preflight hash mismatch, the opened archive surviving a name change,
staging path replacement, and source-path/hash binding of previews. The
previous eight filesystem regressions cover linked ancestors and
leaves, FIFO reads, exclusive file/directory collisions, and refusal/retry of
redirected rollback. The updated primitive module passed a Linux arm64 Docker
smoke (including identity across rename, a collision, source substitution,
and hard links) and a Windows MSVC compile check; these are not desktop release
tests.
Playwright also checks all eight views in light and dark themes. Publication
reconciliation adds seven Rust tests and nine browser cases for read-only
recovery, insufficient or mismatched evidence, protection against stale
observations, and renewed confirmation. Only anonymous provider tool metadata
was fetched live; no authenticated calls or writes were made. Diagnosis
adds 16 Rust checks and six browser checks covering UTF-16 and explicit legacy
decoding, BG3 UUID evidence and limits, read-only journal/Agent behavior, and
source status and encoding selection in the UI. No games were launched.
Modrinth and Thunderstore
version browsing adds provider contract checks for identity, pagination, source
status and read-only Agent/MCP behavior
and 18 browser checks for paging, exact selection, stale replies, and narrow
layouts in both themes. The public metadata endpoints were checked read-only;
no live download or installation was performed. The earlier Modrinth
install-journal review has ten Rust checks (including exact matches
without status reconciliation, replacement rollback, provenance, and read-only
Agent/MCP routing) and 21 browser checks across the three viewports. The browser
tests cover exact upstream lookup, stale replies, conflicting records, and no
implicit install. The version replacement and scoped Nexus cases are included
in the totals. Download origin coverage includes identical repeated downloads,
immutable record conflicts, exact deployment after reopening the database, and
legacy/local fallback. The
browser cases distinguish source IDs from display versions in the installed
table and exercise long IDs in the plan preview. Earlier Modrinth
coverage adds four offline provider contract tests and nine browser cases, with
a separate read-only check of the public version metadata endpoints; no Mod
files were downloaded. The earlier 3DM
coverage consists of 10 Rust provider contract tests, 3 source-tool tests, and
9 browser cases for missing keys, manual fallback and explicit download without
implicit install. The provider fixtures are not live authenticated results.
Previous focused coverage includes 19 import tests, 12 `web_search` tests, 2 `web_tools` tests,
16 publication-state tests and 17 remote-publication tests. Unknown-game coverage
adds 16 backend tests, 4 Agent tool tests, an SSE approval-rejection integration
test covering both review modes and 12 UI cases across three viewports.
Repeated regression testing also exposed a publication metadata initialization
race that could append the initial title to the first input. Both publication
and identity forms now initialize before accepting input. The publication suite
was repeated ten times across all three viewports: 90/90 checks passed.
These are development checks, not release acceptance; no live
Brave request or authenticated publication was exercised in this round.

`cargo check --no-default-features`, Clippy with warnings denied (`--all-targets`)
and `cargo fmt --check` passed on the same tree.
Earlier unsigned macOS debug `.app` testing covered Minecraft
fixture recognition, registration, file reading, line-numbered log diagnosis
and trash restoration.

Additional release smoke evidence reported on 2026-11-18: an unsigned macOS
arm64 release `.app` was launched. In the already-registered
`output/native-smoke/minecraft` fixture, a local archive selected through the
file picker was imported to staging with approval transaction
`txn_04a8bf623fe5494eb6b232e98b335f39`, then installed only after separate
approval transaction `txn_15c539fddfce439d945708ccca2b5762`. Game detail showed
one installed and one enabled item. This used an inert local fixture only; no
real mod was downloaded or published. The fixture JAR SHA-256 was
`bc0ec11c0a3c912ea0e615a5f5197156376efca2027fa54a30352bc501caad36`; the
original, staged and installed fixture bytes had the same hash.

The 3DM build produced macOS arm64 `.app` and `.dmg` bundles. The default
bundle lacked a sealed resource signature, so the same binary was rebundled
with Tauri's ad hoc identity (`-`), without Apple Developer ID or notarization.
The earlier resulting `.app` is 37.00 MiB and the DMG is 13.90 MiB. A current
unsigned arm64 `.app` rebuild completed on 2026-11-19 at 38.58 MiB. CI now has
a macOS launch smoke that uses an isolated home, checks SQLite `quick_check`,
and verifies six embedded plugins. `hdiutil verify`
reported `VALID`; strict `codesign` verification passed both in the mounted
read-only image and on a temporary copy. The copied executable matched the
release app's SHA-256. A direct launch from that copy under an isolated `HOME`
remained running for six seconds, initialized a fresh database (`quick_check`
returned `ok`) and seeded six plugins. The process was then stopped and the
image detached. Nothing was installed into `/Applications`. Gatekeeper's
`spctl --assess --type execute` returned `rejected` for the ad hoc build. This
is local startup evidence, not normal quarantined Finder launch or signed
distribution acceptance, and it does not repeat the earlier game workflow.

3DM-build DMG SHA-256 (not the fixture JAR hash; predates Modrinth dependency support):
`086821d17cc1d1f16e2d95ab717d0a2971d4611cd6c93b7ecbd5103d816eb37c`.

## Remaining Work

- Key-authenticated 3DM REST and real archive acceptance remain pending. Its
  public MCP schema and read-only Mod metadata were inspected, but no download,
  login or authenticated REST request was performed. Live Brave/model/provider
  acceptance remains pending outside the verified anonymous Modrinth and
  Thunderstore flows.
- Verify third-party activation and stronger archive provenance. The Fabric
  smoke observed a controlled fixture entrypoint; it did not start a playable
  server or client. Diagnosis compares supported direct-child JAR metadata and
  tracked paths with the historical selection, but cannot establish which bytes
  the loader used or infer manual installation. Fabric locales other than
  English and Simplified Chinese, and other loaders, need independent evidence.
  The Modrinth graph-review IPC checks manifests for exact selected records; the standalone graph and direct comparisons do not examine files.
  Automatic update selection, automatic shared-file repair/reconciliation,
  loader activation, condition-aware FOMOD selection and game-specific
  ABI/dependency solvers remain open. Browsable version lists
  support Modrinth and Thunderstore; direct dependency declarations are available
  for Modrinth and Thunderstore. New installs
  using completed downloads carry exact catalog version IDs. Explicit reviewed
  replacement works for an exclusively owned deployment, without guessing
  version ordering or source compatibility. Nexus now scopes new project IDs
  by game and does not replace based on older bare numeric IDs.
  Partial-restoration state is implemented; complex restoration/update workflows
  still need coverage and reconciliation.
- Authenticated live Mod 客栈 upload, publication and read-only reconciliation
  verification. Early upload steps, missing IDs and inconclusive provider
  evidence still require manual inspection; none authorize retrying a write.
- Game-specific effective load order, PAK/BA2/BSA and non-loose plugin
  dependencies, further legacy log formats, and actual game-launch
  verification. Bethesda diagnosis now reads bounded TES4 `MAST` declarations
  from exact enabled loose plugins and reports the declared one-based enabled
  order plus disabled or late-listed masters. It still does not verify MO2/VFS,
  archive contents, ESL slots, LOOT rules or live loader state. Current BG3 app
  evidence remains limited to bounded internal LSX checks.
- Verify native Windows parent traversal, archive staging and recovery, and
  extend filesystem race defenses to remaining directory walks, nested volumes,
  and the Unix source-leaf check-to-rename interval. Add native crash/power-loss
  testing and a guided recovery path for legacy journals without identity
  evidence. Stronger cross-platform containment for external stdio MCP processes
  remains open.
- Complete macOS release acceptance: CI now covers isolated packaged-app launch
  and fresh SQLite/plugin initialization, while a temporary copy from the DMG
  booted locally. The ad hoc build was rejected by Gatekeeper, so acceptance
  via normal Finder install/launch remains pending. The fixture smoke does not
  cover crash/disk-full testing or general production archives.
- Run the Windows installer jobs and verify on a clean Windows 11 desktop.
  Linux ARM64 `.deb`
  and extraction-mode AppImage passed the isolated WebView smoke above; native
  FUSE launch, normal desktop acceptance and Linux x86-64 packages are pending.
  Final icon/signing decisions also remain open.

All requirements in the original approved plan remain in scope.

## Desktop game card actions (2026-11-19)

- Implemented desktop IPC commands `update_game`, `remove_game`,
  `open_game_location`, and `launch_game`. The Agent and inbound MCP registries
  exclude these commands. The existing opener capability is sufficient.
- Editing preserves game identity and unrelated metadata. A custom launch path
  is checked on save and launch. The macOS `.app` path resolves its declared
  bundle executable. A valid custom executable takes priority over the numeric
  Steam App ID. Discovery and launcher metadata continue to be inert.
- Removal atomically drops the registration and its user-root mappings and
  detaches conversations. Shared root registrations, history, and disk files
  are retained. Active installations, unfinished reviews, and unrecovered file
  transactions block removal. Abandoned previews that remain `planned` can
  still block removal because the existing plan lifecycle has no general discard
  command.
- Directory opening resolves the game root or the first existing guarded Mod
  directory in adapter order. macOS Finder reveals a resolved `.app` directory
  as a package. Directory lookup creates no folders.
- Windows verification passed all 16 new offline Rust tests, Rust formatting,
  and strict all-target Clippy. The library suite reported 607 passed, 5 failed,
  and 5 ignored with `TEMP` and `TMP` outside AppData. The same five failures
  reproduced before the change (591 passed, 5 failed, 5 ignored).
- Coverage includes name and path validation, Windows junction escape, launch
  selection and revalidation, adapter root ordering and explicit registration,
  installed Mod and pending review checks, atomic database rollback, shared
  roots, and retained file contents. Native Finder, Explorer, Steam and game
  launch, macOS bundle behavior, and Linux execution were not exercised. Path
  validation and the OS open or spawn are separate operations, so same-user
  filesystem replacement can still race them.

### Agent 对话 UI（2026-11-19）
- Agent 对话页已按传统桌面 Agent 产品重排：持久工作区/最近对话侧栏、宽画布式消息区、顶部会话信息和底部悬浮式 Prompt Input；亮色/暗色主题共用语义 token，窄屏保留横向会话列表。
- 使用 shadcn/ui AI Elements 的 Trace 组合承载 Reasoning 与 Tool trace，并新增本地可组合 `trace.tsx`、`code-block.tsx`，不切换项目现有 `new-york` 风格，也不重复实现全局审批弹窗。
- Agent 状态、流式回复、权限策略、思考等级、token usage、错误回放、原生链接打开与现有 Tauri API 保持不变。2026-11-19 验证：`yarn typecheck`、`yarn build`，以及 Agent/引导/工作流相关 93 个 Playwright 用例在 desktop-1280、desktop-1440、narrow-390 通过。
