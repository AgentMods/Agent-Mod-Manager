# Agent Mod Manager implementation status

Updated: 2026-11-24. The desktop release target is `1.0.0`. See the
[Chinese release notes](releases/1.0.0.md) for user-facing features and
installation limitations. This page records implementation and verification
evidence; it does not establish that the release workflow or CI has passed.
The broader project scope remains active.

**macOS package verification (2026-11-24).** CI copies the app from the DMG,
detaches the image and launches the installed copy from a canonical path. Tauri
rejects executable paths containing symlinked components, including `/var` in
the runner’s temporary directory. The smoke checks SQLite initialization in a
fresh home directory.

**Windows package verification (2026-11-24).** Both installer formats installed
successfully on the disposable Windows runner. Elevated WebView2 hosts ignore
the environment flag the smoke used to enable debugging. The elevated test
now scopes a temporary loopback debugging policy to the installed executable
and removes it after the smoke.

**Nexus Mod downloads (2026-11-24).** The stock Nexus MCP can now save files
whose GraphQL `uri` is an opaque storage path. A public Phantom Blade Zero
file (Mod 266, file 623) supplied the regression example. The bundled provider
uses a safe original archive name when available and otherwise combines the
file identity with its detected archive extension. Nine offline provider tests
cover filename selection and the stdio download flow, including account
authorization, mirror renewal, checksum verification and duplicate destinations.
The Windows x64 NSIS package passed its archive test, and its extracted
runtime resolved the patched wheel offline. The desktop MCP and incoming
archive tests passed on Windows. Live testing used public file metadata. The
archive transfer test used a fixture mirror.

**Desktop languages (2026-11-24).** Settings → Appearance and storage has one
Language selector for the interface and newly queued Mod organization jobs.
It follows system preferences by default, falls back to English, and persists a
manual choice across restarts. Supported languages are Simplified Chinese,
English, Japanese, Korean, German, French, Spanish, Brazilian Portuguese,
Russian and Italian. The SQLite migration carries forward a supported legacy
Mod language preference. Previously queued jobs keep their recorded language.

Chat honors an explicit language request, then the language of the user's own
current message. For attachment-only or ambiguous input it consults previous
authored messages and falls back to the effective app language. This also
applies to questionnaires and resumed turns. The UI can switch language while
a chat draft or a dialog is open. Dates, counts, app-authored scan warnings and
known native errors use the effective locale. User content and diagnostic
paths retain their original text. The Windows NSIS wizard selects from the same
ten languages using the Windows display language.

The production build validated 1,238 UI messages and 711 native error details.
Twelve focused checks exercised plural forms, confirmation text and native
error rendering. On Windows the full Rust library suite passed 1,163 tests
with 15 ignored. All-target Clippy and Rust formatting also passed.
Playwright passed 1,022 checks with four skipped, covering all ten choices,
reload persistence, fallback, drafts, open dialogs and narrow layouts. The
final x64 NSIS package passed its archive test. All ten installer language
resources passed their compiler checks, and the wizard opened in Simplified
Chinese on the test host. Desktop WebView relaunch with a disposable Windows
profile and live model response-language checks remain unverified.

**Windows installer welcome page (2026-11-24).** The NSIS setup wizard now uses
GA artwork, native English and Simplified Chinese text, and the Windows app
color preference. Open wizard pages refresh on a system theme change. A native
NSIS fixture switched both languages between light and dark using a private
registry subtree and navigated through directory selection and completion.
The packaged x64 installer was opened on Windows and exercised through Next,
Back and Cancel. The package smoke job covers installation on a disposable host.

**Archive and Linux dependency security fixes (2026-11-24).** The Rust lockfile
selects `zip 9.8.2` for archive handling. ZIP preflight propagates filename
decoding errors and rejects entries without a validated data offset. The Linux
GTK 3 stack uses `glib 0.18.5` with the two-line `VariantStrIter` fix from
`gtk-rs/gtk-rs-core#1343`, pinned to commit `7e91f1a9`. Remove the Cargo
patch once GTK's dependency constraints accept a fixed registry release.
On Windows, 1,146 Rust library tests passed and 15 were ignored. The old and new
ZIP writers produced byte-identical plugin packages for the comparison fixture,
preserving its preview hash. A Linux release-mode smoke test exercised both
directions of `VariantStrIter` against the pinned patch.

**Desktop application updates (2026-11-24).** Signed desktop builds check the
GitHub Releases manifest on launch. Settings offers a manual check and shows
progress and errors. A verified update waits in a temporary file until normal
process exit, when installation waits for active file transactions. The installer
leaves the app closed. The tag workflow assembles a draft release with separate
signed payloads for NSIS, MSI, AppImage, Debian, and both macOS architectures.
Windows verification produced MSI and NSIS updater signatures with the Tauri CLI.
Ten Rust updater tests passed, including signature rejection, staging errors,
and shutdown races. Fifteen Playwright checks passed across three viewport sizes. A release assembly
test verified all six manifest entries and rejected a modified payload. A live
upgrade between two published versions still needs a public signed release
channel. See the [release setup](../README.md#维护者发布流程).

**Windows MCP startup windows (2026-11-23).** Restoring saved stdio connections
could open a terminal for each bundled uvx or npx launcher. Bundled and custom
MCP commands now use the same Windows no-console creation flag as the browser
helpers. The desktop still restores the selected connections through stdio.
The packaged npx launcher completed an offline MCP handshake
and listed a fixture tool. A local wheel started by the packaged uvx launcher
echoed piped input and reported no console window handle.

**Agent and MCP game-library controls (2026-11-23).** Desktop conversations and
clients of the authenticated local MCP server can list registrations, add a local
game directory, edit the display name or launch program, remove a registration,
launch a game, and bind an installed adapter. Listing supports pagination and
respects the desktop conversation's workspace. External calls identify existing
games by ID. Updates, removals, registrations, bindings and launches recheck the
reviewed target at execution. The regular approval matrix applies to manager
changes and game launches. Removal preserves files and checks active installs,
file recovery and pending reviews. Successful changes refresh the desktop library.
Windows verification passed 1,135 Rust library tests with 15 ignored, all-target
Clippy, Rust formatting and the frontend production build. A loopback model test
removed two selected registrations in one turn while keeping their save files
and a third registration. Authenticated MCP tests exercised all five operations
under each permission mode using a temporary library and a fixture executable.
Thirty Playwright checks across three viewport sizes covered game and adapter
cache refresh, including delivery through the persisted Agent event log.

**Chat links and app logo (2026-11-23, Windows).** Pasted URLs and Markdown
links wrap within chat bubbles and stay underlined in both themes. User links
inherit the bubble text color. Assistant links use the theme accent foreground.
The sidebar and onboarding header use the dark GA mark on light surfaces and
the white mark on dark surfaces. The frontend build and 39 Playwright checks
for message actions, theme switching, motion and sidebar state passed across
three viewport sizes, including link contrast checks in both themes.

**Desktop usage counts (2026-11-23).** Release builds send fixed startup and
feature events to the self-hosted Aptabase instance at `analytics.gloscai.com`
with OS and application versions. Desktop setup
counts a successful start once. Managed Mod installs count after the transaction
commits across desktop and Agent paths. Other feature events count accepted
desktop actions. Each process keeps its daily session ID and outbound queue in
memory. Debug builds, stdio proxies and package smoke runs skip collection.
See the event names, counting rules and opt-out in [the README](../README.md#匿名使用统计).
The Windows release binary built successfully. The Rust library suite passed
1,124 tests with 15 ignored, including local HTTP payload checks. All-target
Clippy and Rust formatting checks passed.

**Removing Mods with overlapping legacy installations (2026-11-23, Windows).**
Installation inspection can identify the last writer of a shared path by matching
file identities and fingerprints in the committed copy and backup journals.
The Agent and transaction history recheck the installation before rollback.
Batch removal rechecks selected installations after recovering the preceding
files. Ordinary installation previews and commits now enforce the ownership
checks already used for library installations. Restoring a managed uninstall
checks all files its record would reclaim, including unselected trash items.
Windows tests covered two and three overlapping installations, identical bytes
with distinct file identities, changes after approval, and removal of both list
records with the original game file restored. The 14 desktop batch-action tests,
frontend production build, all-target Clippy and Rust formatting checks passed.

**Library archive member installation (2026-11-23, Windows).** The generic file
planner directs library operations to the artifact-based installation tools.
The library matcher identifies existing active deployments. Both planning and
apply reject whole-archive mappings to native binaries and collisions with
paths owned by another active install, including missing files. Generic trash
operations check active ownership for files and parent directories at both
stages, including trash plans approved before upgrade.
A ZIP fixture exercises member selection, deployed plugin bytes, preservation
of the original archive and transaction rollback. The Windows Rust library suite
passed 1,104 tests with 15 ignored. All-target Clippy and the Rust formatting
check passed.

**Automatic post-install Mod organization (2026-11-23, Windows).** The desktop
runs the built-in `system-mod-organize` skill after a managed installation
commits. It uses the configured model in a separate, tool-free request. Schema
v23 queues the exact install ID in the installation transaction. On startup the
worker also picks up previously installed records with archive filenames,
missing tags or fallback classifications. The active install chat may finish or
close while organization runs. An open Game Details view refreshes after the
localized display name and tags have been saved.

The worker uses recorded source titles and meaningful relative component names
when download filenames are opaque. It preserves manual tag assignments and
leaves names alone when the available evidence is insufficient. Transient
failures retry with backoff. Disabling the skill pauses pending work, and a
completed record stays complete across restarts. The shared Language setting
(under Settings → Appearance and storage) controls the interface and the
language of newly queued Mod organization jobs. It defaults to the first
supported system language, or English. Earlier supported Mod language
overrides migrate to this setting; changing it does not alter queued or completed
jobs. Agent chat still follows the current prompt.
Game Details shows customizable color tags before the Mod name and supports
filtering by tag. Organization updates manager metadata only.
A local model fixture exercised both a new approved installation and startup
repair of an existing hash-named record. Nine UI event tests across three
viewport sizes covered automatic refresh, late listeners and game isolation.

**Per-conversation Agent game workspaces (2026-11-23, Windows).** The sidebar workspace
selector (chat header on narrow screens) stages single or multiple registered games,
dynamic all, or explicitly none. Schema v21 preserves previous game bindings and
keeps an empty scope empty when the last selected game is removed. The next turn
uses the saved selection; changing it does not alter the transcript, filesystem access,
or approval policy. Active turns and archived chats cannot change scope. The frontend
production build and 15 workspace-specific Playwright checks across three viewport
sizes passed; the dark desktop and light narrow layouts were also inspected. The
12 workspace-specific Rust tests passed, including migration, game removal,
concurrent turn start, download classification and installation targeting.

**Batch Mod actions (2026-11-23, Windows).** Game Details → Mod management has an
opt-in selection mode for installing, uninstalling and removing Mods. Mixed selections
skip ineligible rows; repeated archive targets are installed at most once per dispatch.
All eligible Agent targets from one click share one conversation and one turn,
with one review panel and separate tool receipts and list checks for each target.
Chat displays the generated install, uninstall and removal requests as cards in
both themes. Each card lists the game and target Mods, with exact IDs in an
expandable section. Missing names fall back to record identifiers. Copy and
edit use the original Agent message.
Local-only list removals create no conversation. A retry after a failed turn start
reuses the draft conversation for the same action and target set on the game page.
Deletion previews the targets before submitting. Installed Mods follow their scoped
Agent workflow and require both a removal receipt and list readback; inactive records,
plans and unused archives use the existing desktop list-removal command. Other visible
records tied to the same archive are retained when deleting a record.
The production build passed. Across three viewports, 36 batch-action and 51 removal-
tracking Playwright checks passed. A nine-Mod deletion in the built frontend issued
one conversation and one turn and produced one entry in the removal group, using
mock IPC. A Rust integration test ran the real Agent runtime with a loopback model,
removed two fixture Mods in the same turn and verified both sets of receipts and files.

**Collapsible task conversations (2026-11-23, Windows).** Recent Agent chats group
Mod installation, uninstallation, list removal and adapter authoring separately.
Ordinary and pinned chats remain directly accessible. A group takes its newest
chat's position; opening a chat reveals its group, and manual disclosure choices
are remembered locally. A collapsed group still indicates running or waiting work.
Schema v20 stores the original category and backfills recognizable historical
operations once, without changing chat titles, messages or archive state. The
frontend production build, 22 Rust history tests and 36 related Playwright cases
across the three viewport projects passed.

**Desktop plugin list actions (2026-11-23, Windows).** Catalog install and row-level
removal passed 57 Rust catalog/workflow tests, including duplicate clicks, registry
failure rollback, restart, and reinstall. Thirty Playwright cases across three
viewport sizes covered the two lists, optional details, explicit game binding,
retry behavior, and existing installed/draft timestamps. The frontend production
build passed. The tests used local fixtures, not the live public catalog.

**Mod download location (Windows, 2026-11-22).** The stock Playwright browser saves
files to `downloads/Playwright` under the active storage directory. Schema v19
updates unchanged legacy presets; moving storage rebases the stock output folder
and expires its old launch consent. Custom output paths and files in the former
system Downloads folder are left untouched. The four bundled uvx source presets
set their own download-directory environment variable to `downloads/incoming`
when launched. A real Chromium download from a loopback page landed in the stock
managed directory; offline tests also cover relocation and filing a ZIP in `Mods`.

**Managed Playwright browser (Windows, 2026-11-22).** The stock MCP no longer
requires system Edge or Chrome. The packaged CLI downloaded Chromium into an
empty cache. With the downloader proxy then set to an unreachable address, the
native MCP connection reused that build, opened a local fixture page and saved
the expected file bytes in its configured output folder. No live mod site or
account was used in this test.

**Background installation switches.** In Game Details → Mod management, the
status switch sends an Agent install, reinstall or uninstall for that exact
archive or install record. Each pending switch is disabled independently.
File-plan approvals and installation questions can be answered on Game Details;
changing pages does not cancel the task. The switch reads back the native
install list after the turn ends and checks the scoped tool receipt (including
uninstall by rollback). A missing receipt or mismatched list state is not
success; retrying a failed status read does not send a second install command.
Removing a Mod from the management list remains a separate action.
Frontend production build, 177 related Playwright cases across the three viewport
projects, and all five offline Rust Mod-library Agent flow tests passed.

**Single Mod management table (2026-11-22).** Game Details lists install records
and still-uninstalled archives in one table. Archives are matched by artifact
ID, not display name: installed packages do not get a duplicate archive row,
and new downloads/imports remain available to install without leaving the tab.
Uninstalling does not discard the archived package or its reinstall action.

**Installed, uninstalled and removed Mods (2026-11-22).** Game details distinguishes
uninstalling from removing list membership. The desktop can hide unused archives,
uninstalled records, or individually cancelled previews. Deleting a deployed Mod
runs its scoped Agent uninstall/rollback before the removal step. Game Details
uses the same permission selection as chat for install, uninstall and removal,
including a choice made before leaving the composer. With no explicit choice,
the new conversation inherits its normal backend policy. SQLite retains
ownership and transaction history for restoration. Ordinary uninstall keeps both
the record and its library entry.

**Removal when all deployed files are already gone (2026-11-23).** On an explicit
delete, a verified manifest of exclusively new, now-missing files can be removed
from management without an uninstall or rollback. Approval is bound to the game,
install and inspected state; execution rechecks the filesystem under the writer
lock. Refreshing the list no longer resurrects the record or counts it as enabled.
The original archive and file journal are kept. Partial loss, modified files,
overwrites, shared ownership and replacement chains are not eligible.

**Agent permission audit.** Chat, retry/edit, questionnaire follow-ups and Game
Details use the same turn selection. The model receives the resolved mode and
is told not to ask for duplicate execution consent; genuine choices still wait
for the user. A historical turn without a recorded mode is not labelled Full
Trust. Direct inbound MCP tools use only the host global mode, record it at
start, and will not commit using automatic consent revoked during preparation.
Managed Mod Inn download receipts also refresh the library after a Full Trust
transfer, without relying on a manual approval event. Windows verification passed all 1,033
non-ignored Rust library tests (15 ignored) and strict all-target Clippy. UI
regressions cover per-turn inheritance, retry/replay, background status, real
questions vs approvals, and the install/uninstall switches in Full Trust.

**Game-classified Mod library and Agent installation (2026-11-22).** New downloads
are filed under the active storage root's `Mods` folder. Each game category can
be used before its installation directory is registered. The Agent checks local
layout, README and FOMOD evidence before reading the author's project description.
Game Details starts an Agent turn for the selected artifact or installation. The
library installer uses FileEngine copy plans with a persistent per-file manifest;
uninstall uses that manifest and the recorded overwrite or replacement backup.
Existing download folders are not automatically imported.

Windows verification passed 934 Rust library tests with 13 ignored, strict
all-target Clippy, the formatting check and the frontend production build. Four
loopback Agent flows cover local install/uninstall/reinstall, cancelled approval,
author information from the source MCP and fallback to a Playwright MCP fixture.
The focused UI run passed 63 cases across three viewport sizes. The full UI run
passed 556 cases, skipped two and failed nine. Six failures require the hidden
History navigation item and three expect an 80-character game display-name limit.

**Long-running Agent tasks (2026-11-22, Windows).** The model/tool loop continues to a final
reply or cancellation. Regression fixtures completed 41 tool rounds in one turn and
stopped a different turn during its 33rd tool execution. Streaming and buffered
completions accept more than 64 calls; the registry can install more than 256 tools.
MCP discovery follows advancing cursors and reports malformed, repeated or stalled
pages. Model retries use cancellable backoff for interrupted transports, and context
overflows may retry a progressively smaller prompt. File reads, scans, diagnostic
logs, web search results and publication history provide access to later pages.
The integrated Windows run passed 941 Rust tests, with 13 explicitly ignored.
The related browser suite passed all 117 cases across three viewport sizes.

**Runtime boundaries for long tasks (2026-11-22).**

| Operation | What can still end or limit a request |
| --- | --- |
| Model and MCP | The selected provider controls its context window and its request and output limits. Memory for model response buffers, MCP catalogs and frames depends on free memory and active requests. The rmcp HTTP JSON collector accepts at most 16 MiB per response and 64 KiB per HTTP error body. An MCP that has no pagination must fit its result into one transport message. Each discovery page has a 30-second response deadline; ordinary tool execution is cancellable. |
| Search | A browser-helper query uses a 256 KiB request envelope. A web search page returns at most 64 KiB, and a cursor continues results from the same query. Upstream empty or repeating pages terminate traversal. |
| Files and diagnostics | Text and directory tools read in pages. Cursors for long scans expire when idle. Particular binary and XML parsers may mark oversized metadata or individual records as limited. |
| Downloads and archives | Transfers and extraction require disk headroom and check cancellation. The default HTTP read-idle interval is 30 minutes. ZIP index metadata and 7z headers/dictionaries have parser bounds. A solid 7z stream needs its earlier dictionary state to resume. |
| Approvals and local writes | Plans still require the permission mode's approval and use validated paths, reversible filesystem transactions and backups. Local drafts and text plans can exceed the old 1 MiB threshold; inbound MCP and remote publication endpoints have their own transport and validation rules. |

**Agent directory reads and long conversations (2026-11-22, Windows).** The desktop Agent's
`read_file` returns a directory preview when given a folder, including child paths
that it can read next. Text files still return numbered lines. Old model budgets are
ignored. The runtime resolves the selected model's window and requests a remote
summary at 80% occupancy, including during a single turn with many tool calls.
The latest provider prompt count, when available within a turn, is combined with
an estimate for newly added messages. The estimate includes tools on a fresh request.
A provider-reported context overflow can also trigger compression when the window is
unknown. Long inputs are summarized in chunks using the selected model. SQLite stores
the summary and cutoff together so later turns resume from the checkpoint while the
full original transcript remains available. Editing or rewinding invalidates affected history.
Windows verification passed 809 Rust library tests with 12 ignored, strict Clippy,
formatting and the frontend production build. Loopback model tests covered prompt
compression, tool results, attachments, model switching and retry after a provider
context rejection. The settings and workflow Playwright run passed 129 cases;
six existing history-navigation cases could not find the History button.

**Native Agent file paging (2026-11-22).** `read_file` accepts a 1-based
`start_line` or a 0-based `byte_offset`, and returns `next_byte_offset` when
another text page exists. Each page remains capped at 2,000 rows and 256 KiB
of source bytes. Long lines keep their absolute line number across fragments;
page boundaries do not split UTF-8 characters or CRLF. For directories, use
`offset` and `limit` with either tool and follow `next_offset` (make a fresh
first-page request if the contents change). A path-only `read_dir` returns an array
for a complete listing, or a page with `next_offset` for a longer directory.
Inbound MCP and desktop IPC keep their original schemas and results.

**Installation rule research (2026-11-21, Windows).** When creating or changing
installation rules, the desktop Agent uses connected Mod-source MCP tools to select
release files with distinct installation layouts. The game Skill records the
sample, file ID or version, observed paths, dependencies and installation steps.
The authoring guide reserves automatic mappings for verified layouts. Configuration
edits and unresolved layouts are documented for manual review. The existing `extract_preview` tool
accepts optional `offset` and `limit` for sample inspection. Each page validates
the full archive and reports the total entry count and next offset.

A live anonymous Nexus Mods MCP check for Black Myth: Wukong retrieved author
instructions for a CSharpLoader plugin and a preset appended to Engine.ini.
See the release pages for [Easier Dodge, file 5120](https://www.nexusmods.com/blackmythwukong/mods/1571?tab=files&file_id=5120)
and [Pure Clarity, file 5111](https://www.nexusmods.com/blackmythwukong/mods/1564?tab=files&file_id=5111). The check covered the authors' instructions and release
metadata. Archive contents are covered by local fixtures. The public 3DM MCP
advertised its tools but returned a service error for the game and Mod queries.

A loopback Agent and MCP fixture covers discovery, sample previews, draft creation
and validation. Under the former 32K budget, the conservative unknown-model
token estimate could exceed the budget during a multi-sample turn.

**Bing web search (2026-11-21, Windows).** The packaged Playwright helper
returned web results for `Minecraft official website` through the Rust search
entry point. The agent and opt-in game identification share this helper.
Existing game identification evidence retains its recorded source. Eleven
offline browser tests covered parsing, redirects, blocked prefetches, error
pages and cancellation. The 18 focused desktop UI cases for identification and Skill settings passed across
three viewport sizes. Six broader workflow cases still expected a history
navigation button that was absent from the current UI.

**Game library scan dialog (2026-11-21).** Opening the dialog scans common
locations. Directory selection, single-game inspection and scan results share
that dialog. Adding a game marks its row as managed and refreshes the library.
Players can continue adding candidates or close the dialog and open a game card.

**Cloud adapters during game discovery (2026-11-23).** The Library scan dialog
and onboarding compare candidates locally against the anonymous manager-plugin
catalog. A unique Steam App ID or exact normalized game name/alias can suggest
an adapter. Both scan views show the matched release's `gameName` as the row title
before registration and keep the plugin title in the install details. For a legacy
catalog, the scan uses a distinct catalog game name when available and otherwise
keeps the discovery name. Browsing the scan does not download or activate a package. Adding
a matched game selects a specific plugin ID and release ID. The desktop checks
the published files, re-probes the canonical directory against all enabled adapters,
and commits the game binding alongside the journaled package installation. A
failure to register rolls back the new package as well. Exact already-installed
packages can be reused; disabled or differing local packages are never replaced.
Absent/ambiguous catalog matches and offline use retain local registration. The
separate opt-in web identification flow still does not authorize installation.
Verification: production build passed; 40 focused Rust tests passed (one
large-scan test ignored), and 72 Playwright scan/onboarding cases passed across
the three configured viewport sizes.

**Windows Rust test directories (2026-11-21).** The local and CI test launcher
places each run under `src-tauri/target/test-tmp/` and removes its directory
when Cargo exits. The library suite passed 738 tests with 8 ignored. A
filtered SQLite test and a Cargo argument error also exercised cleanup.

**External game adapters (2026-11-21, Windows).** New profiles start with an
empty adapter registry. Startup retires packages registered by older releases
as built-in, moves their managed directories into `backups/retired-adapters/`,
and clears their game bindings. The upgrade regression checks that installed Mod
records and external bindings survive, local edits are backed up, and a retired
slug can be installed through the external package workflow and read after a
restart. Game discovery tests cover registering a game with an empty adapter
registry. The Windows Rust library suite passed 738 tests with 8 ignored.
Playwright covered external installation from the empty package center at 390,
1280 and 1440 px. Strict Clippy and the frontend production build passed.

**Agent adapter binding (2026-11-21, Windows).** A game card's authoring chat
can create, validate, install and bind an external package in one turn. The
`set_game_adapter` tool uses the conversation's registered game, or an explicit
game ID in a general chat. It shares the library's disk-marker and pending-task
checks and revalidates the reviewed binding at commit. Full Trust can complete
the operation directly. The other permission modes request approval in chat.
The authoring chat prefills the game library display name. When the user edits it,
the binding tool accepts the requested name in `game_name`, previews both names,
and saves the name and binding in one transaction. Successful tool results refresh
the library and game detail when the turn ends.
A loopback model fixture covered the full authoring flow under the default
context budget, and approval tests covered applying an existing package and
rejecting the change. Twelve additional Playwright cases across three viewport sizes
covered the name field, validation, approval text and library refresh. The Windows
Rust library suite passed 789 tests with 12 ignored.

**User-visible filesystem paths (2026-11-21).** Game discovery, registered games,
file browsing, storage settings and native tool previews display ordinary Windows
paths such as `E:\Games` and `\\server\share`. Files that require the verbatim
namespace keep their original spelling. Database roots and transaction journals
retain their canonical paths. The Windows Rust library suite passed 713 tests
with 8 ignored. Across three viewports, 85 UI tests for paths, discovery, file
browsing, library actions and storage settings passed, with 2 skipped. Strict
Clippy and the frontend production build passed. A Linux container also exercised browsing path helpers against real filenames
with case differences, embedded backslashes and trailing spaces.

**Game detail file tree (2026-11-21, Windows).** The Files tab uses the shadcn
File Tree pattern with on-demand folder reads and search across expanded folders.
Refreshing reloads the expanded branches and the selected preview. The browser
can switch between the game directory and its registered user roots. Mocked IPC
checks across 390, 1280 and 1440 px cover expansion, keyboard selection, search,
root switching, refresh, and preview cleanup after a reviewed trash operation.

**Desktop Agent filesystem access (2026-11-21, Windows).** Six local file-tool
regression tests passed, including listing a registered game's parent, creating
and overwriting external UTF-8 files, approval, credential exclusions, rollback,
and restart recovery. The loopback MCP test confirmed that the managed-only
planner does not advertise `write_text`. Formatting and strict Clippy passed.
With TEMP/TMP outside AppData the full Rust suite had 664 passed, 4 failed and
6 ignored. The remaining failures covered two diagnostics path cases, the game
card's unregistered Mod directory, and portable import filename validation.

**Agent window responsiveness (2026-11-20, Windows).** Turn start, retry,
event replay, cancellation and approval IPC no longer wait for database or
runtime locks on the window thread. Startup repairs interrupted tool calls from
protocol metadata only; the background task builds the prompt once. The former retry implementation copied prior completed events. On a
synthetic in-memory SQLite fixture with 8,001 events, optimizing that copy
reduced its duration from about 9 s to 6 ms.
Live text batches within 32 ms, terminal/approval events flush immediately,
and unchanged Markdown and completed reasoning traces stay stable.

The locked-database Tauri dispatch regression, stalled-model cancellation,
interrupted-stream retries and all 30 focused browser cases passed. The frontend
production build and Rust all-target check passed. The broader Windows runs
reported 632 Rust tests passed, 5 failed and 5 ignored (temp outside AppData),
and 261 browser tests passed with 9 failures in history navigation and the game
file-tab workflows. An isolated Windows/WebView2 build also sent a real HTTP
request to a loopback SSE fixture that withheld the first token. Its native
window answered all 20 `WM_NULL` probes (maximum 9 ms); switching to the game
library, returning to chat, and stopping generation closed the stalled request.
No external provider, user credentials or existing conversations were used.

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
- The desktop Trash list uses compact, expandable rows and paginates after
  search/game filtering (10, 25 or 50 items per page). The header checkbox
  selects only the current page; selection persists across pages and filters.
  Empty Trash explicitly confirms a snapshot of all unrestored items across
  games/pages, not just the filtered view. The preview is paginated and large
  purges use batches within the existing desktop IPC limit.
- Only `trash::purge_trash` permanently removes files. Turning on the confirmation switch
  enables the desktop dialog’s delete button. Clicking it sends the selected IDs
  and the required `PURGE_TRASH` confirmation over desktop-only IPC.
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
- ZIP/7z/tar.gz checks validate paths, reject links/devices and enforce declared
  versus streamed lengths and actual staging space. Large entries and high ratios
  are not rejected solely on size or ratio. ZIP metadata, tar PAX and padding, and
  encoded 7z headers and dictionary memory retain separate safety budgets. Failed
  extraction cleans its owned stage when its directory identity is unchanged.
  Preflight and extraction share one open artifact, verify its hash through
  that handle, and compare entry lists. On macOS/Linux and Windows, the extractor
  holds a staging directory handle for entry writes. The install preview
  requires exact staged source paths and hashes. If directory identity changes,
  cleanup does not delete the replacement. Staging referenced by a committed
  plan is kept for the transaction engine.
- Game adapters can be installed directly from the desktop catalog. Authoring
  and Agent imports retain the draft and reviewed deployment workflow. Schema validation covers identity, versions, relative paths,
  references and Skill frontmatter.
- `adapter.json` accepts an optional `gameName` for the default game library name. `name` is the adapter title. Older manifests use `name` as their local default. Plugin registration stores both names, and new publication previews retain the game name when assigning a release title. Package upgrades refresh recorded automatic names. Player-chosen names and older registrations with no name provenance keep their existing labels. The game-name flows passed the Windows Rust library suite (1,104 passed, 15 ignored), 39 focused Playwright cases, and a local Mod客栈 MCP publication and download round trip with PostgreSQL 17.
- The adapter manifest provides optional `coverUrl` for the library and package
  details. Missing or unavailable covers fall back to the game placeholder.
  Existing manifests can omit the field. `loadOrder.kind` accepts `none` for games without a dedicated
  load-order format.
- Players can start a prefilled Agent chat from the game library and package
  center or request a new or updated adapter in chat. The Agent can find
  registered packages, read verified package contents, and use the existing
  draft, validation and installation transactions. Local updates keep the slug.
  The desktop library applies
  a selected installed package to a registered game after checking its markers.
  Players can also detach the current package in the same selector, even if it
  is disabled. Detaching keeps the game, its installed Mods and the package
  available for later use. Both changes run the same pending-task checks.
  A standalone, unapproved file preview (such as creating a Mod directory)
  does not block binding or run as a side effect. Pending reviews, approved
  operations on the game, game-linked plans and recovery still block binding;
  removing the registration still checks all unfinished previews.
- In the package center, deleting a draft moves only the selected draft ID's
  directory to the manager's Trash after confirmation. Restoring it makes the
  same draft available again. Installed packages and publication history are kept.
- Agent-authored and Agent-imported drafts move to Trash automatically after
  successful installation or confirmed publication, unless another installation
  or publication still needs that exact draft. Validation alone does not consume
  it. Startup retries missed cleanup; older drafts are eligible only when a
  successful Agent receipt identifies their draft and write transaction. Manual,
  modified and restored drafts are left alone.
- Settings shows six embedded system Skills for adapter authoring, web search,
  Mod downloads, custom commissions, installation and uninstallation, plus
  manually installed Skills in their own section. When a player wants a custom
  Mod or a search finds no suitable release, the commission guide links to the
  Mod 客栈 creation form and helps draft the request. The player reviews the
  bounty and submits the request on the site.
  Game Skills are available from their package details. The Agent can read the
  active system Skills and the selected game's package Skill.
- Desktop Settings uses the 工具 and Skills tabs as the entry points for Mod provider
  workflows. The built-in web search and game identification use Playwright to
  search Bing without a search API key.
- Game discovery: bounded shallow scans, marker confidence with ambiguous
  candidates, structured Steam ACF and libraryfolders parsing, secondary Steam
  libraries, Epic/Legendary metadata, explicit canonical root registration,
  idempotent game state persistence and list/get commands. Discovery does not
  grant write access or execute launcher metadata. In the game library, picking
  a folder scans it immediately; a typed path scans on Enter or “扫描此目录”.
  This scoped scan excludes common launcher locations, can surface unadapted
  installation candidates, and still requires explicit registration. An automatic
  adapter match requires at least 80% confidence and an unambiguous result.
  Other candidates use the launcher or directory name. Adding a scan result
  attaches an adapter only if a fresh directory check confirms the same suggestion.
  A unique full adapter match is labeled “已适配” with a green check.
- First-run onboarding has five steps. Users select a data directory, configure
  the AI model service, enable MCP tools, scan for games and review their setup
  before entering the library. The storage step shares the Settings directory
  picker and migration confirmation. After a migration restart, the guide opens
  at model configuration when the active root has changed, or returns to the
  storage step to show a migration error. Model setup, tool activation and game
  scanning can be deferred. Clicking “添加” on a scan result adds that game directly.
  The completion screen shows the current directory, default model, running tool
  services and library count. The guide supports light and dark themes, a compact
  progress indicator on narrow screens, and reopening from Settings.
  Windows verification on 2026-11-22 passed the production build, 141 Playwright
  cases across three viewport sizes, and five Rust storage/onboarding tests.
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
  the selected model's context window for automatic compression. Selecting a
  model in the composer leaves the saved default intact. Legacy single-level
  settings load as one enabled option.
- The composer remembers the last model, reasoning level and permission policy
  used to send a turn. The running chat shows the permission recorded at the
  start of that turn, not the selection for the next turn. The composer
  preferences are stored in the SQLite `settings` table under
  `composer_preferences` (separate from `AppSettings`, so saving Settings never
  overwrites them) and restored on the next launch. A removed model or a level
  the model no longer enables falls back to the saved default. SQLite lives in
  the app data directory (or the relocated storage root), so the choice survives
  restarts and an uninstall that keeps user data. A reinstall that wipes the data
  directory resets it, like every other setting. Covered by
  `settings::tests::composer_preferences_*` and `tests/ui/composer-memory.spec.ts`.
- The left navigation rail remembers a manual expand/collapse in WebView
  `localStorage` (`amm.sidebar`, same mechanism as the theme). Without a saved
  choice it still follows the window width; an invalid value is ignored. Covered
  by `tests/ui/sidebar-memory.spec.ts`.
- Agent messages expose copy, edit, and retry in the message footer. Edit
  restores the original text in the composer. Sending an edit or retrying a
  reply regenerates from the selected question in the same conversation,
  replacing the previous reply and later turns. Earlier messages, the chat
  title, game binding and pin are preserved. Retry reuses the original
  attachments. The rewind and replacement turn start commit together, and
  live and replayed events carry the cutoff for removing superseded output.
- Agent tool approvals appear inline in the owning chat using the shadcn/ui
  Confirmation pattern. The card shows the prepared request or file plan before
  the user approves or rejects it. After a decision the tool trace shows the
  outcome. A header shortcut opens a chat with a pending background approval.
- The desktop sends a native Tauri notification when a chat task completes
  successfully, asks for an answer, or pauses for tool approval. Replaying
  history does not resend notices. Notification text contains no chat or tool data.
- Agent `ask_user` requests open a questionnaire in the bottom message composer.
  Each step accepts a single choice, multiple choices (`multiSelect: true`), or
  free text. Players can select preset options alongside a custom answer,
  revisit earlier steps, or skip a question. Multi-select replies preserve the
  selected labels in option order, followed by any custom answer. Older
  option lists with a standalone `可多选` hint also use checkboxes unless
  the question explicitly sets `multiSelect: false`.
  Long questions scroll above the step controls. Live questions retain the
  stop-generation action. The approval transport preserves skipped steps as
  `null`. Legacy single questions render as a one-step text card.
  Completed questionnaires collapse to a question count in the transcript.
  Expanding it reveals the questions and answers in order, including skipped
  steps and cancelled requests. Replay restores answers from persisted results.
  Cancelled, expired or entirely skipped questionnaires in an unarchived chat
  offer a full-size **重新回答** button. It opens the same bottom panel and sends
  the answers as a new turn in the original chat. Submitting or cancelling
  restores the composer with its draft text and attachments.
- Composer messages and history edits/retries no longer apply fixed text byte,
  attachment count, per-image, document upload, Markdown or combined attachment
  quotas. Only user-selected bytes take this path. The existing image formats
  and file validation still apply. Historical images count against a context
  retention budget; the current turn's images are not removed to meet it.
  UTF-8 TXT/Markdown attachments reuse their text directly. Other formats use
  the approved local MarkItDown MCP one file at a time. Its conversion result must
  fit one response in the negotiated local MCP transport. The model provider's
  context window, vision support and request limits still apply.
- Conversation rows show just a color-coded status icon at the right, with its
  label available on hover, keyboard focus, and to assistive technology. **进行中** marks
  an active turn; an outstanding questionnaire shows **等待回答** and an operation
  approval shows **等待授权**. A successful turn shows **已完成**; an empty chat or
  a cancelled, failed or interrupted turn shows **等待输入**. The list starts from
  a persisted checkpoint and applies newer events even for unselected chats.
- Explicit user-data root registrations, bound to game and adapter root ID.
  Operations that require registered roots still reject symlink-replaced roots,
  including replacement with a different authorized game or manager directory.
  The desktop Agent's `read_dir`, `read_file`, `extract_preview`,
  `plan_file_ops`, and `apply_file_ops` can use paths outside those roots
  under the app process's OS permissions. Credential/`.env` protections and
  path safety checks still apply. `extract_preview` is a read-only archive
  listing, not extraction or installation. Inbound MCP keeps its existing scope.
- The desktop Agent's native `plan_file_ops` accepts `write_text` (`dst` and
  full UTF-8 `content`) to create or overwrite a file through `apply_file_ops`.
  Planning fingerprints the inline text without writing the destination. A plan
  with `write_text` requires review under `auto_approve`, even for registered
  game paths. Inbound MCP and other generic planners reject this operation.
- The desktop Agent also accepts `compress` (`src`: ordinary folder, `dst`: new
  `.zip` or `.7z`) and `extract` (`src`: supported archive, `dst`: new folder)
  in `plan_file_ops`. RAR4/5 is read-only: listing and extraction accept
  only unencrypted, single-volume RAR archives. The local Mod import picker
  accepts `.rar`; compression does not create RAR archives.
  Compression preserves the source folder name as the archive
  root; extraction preserves the archive layout. Both fingerprint the source,
  leave it unchanged, and require review under `auto_approve`, including within
  registered game roots. A validation failure never publishes a target. The
  Agent rejects protected paths and archive members (including `.env`, `.env.*`,
  `private` and `.amm-retained`), linked source files and ambiguous member names.
  Rollback retains the generated output only if its identity and content still
  match the journal. The existing Agent approval UI and transaction history are
  reused; inbound MCP and other generic planners do not support these operations.
- Adapter-driven install previews and installed-file manifests. JAR/ZIP payloads
  declared single-file stay intact; SMAPI directories retain their manifests;
  declared Data prefixes are stripped only after inspection. Unreal Pak adapters
  and explicit file mappings accept literal `~mods` components in source paths
  and destinations, including archives rooted at `~mods/`. FOMOD archives now
  have a bounded `ModuleConfig.xml` preview in the desktop local-archive
  install flow and the Agent. `preview_fomod` still accepts only archives in
  managed downloads or staging. Folder directives expand descendants from the
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
- App startup handles logging, database initialization, transaction recovery,
  legacy adapter retirement and single-instance enforcement. Rust IPC wrappers exist for current file/trash/snapshot
  and plugin operations; file-changing commands produce plans before apply.
- Rust Agent runtime: OpenAI-compatible chat completions SSE, incremental UTF-8
  and tool-call assembly, complete-stream gate before tool execution, bounded
  memory-aware model transport, a cancellable tool loop, one active turn per conversation,
  cancellation during HTTP streaming and approvals, usage and progress events.
  The desktop Rust task owns each turn; the app shell follows active conversations
  across navigation and replays missing events through the final sequence number.
  A truncated SSE response retries the current completion once in streaming mode.
  If both streams end early (including transport errors), the third and final
  request uses `stream: false` with the same messages, tools and reasoning effort.
  The JSON fallback must finish downloading within the 16 MiB size limit and pass
  validation before replacing the visible draft or dispatching any tool. Failed
  recovery leaves the last draft visible; it never replays earlier tool rounds.
  Windows loopback tests cover truncated HTTP bodies, missing SSE terminators,
  successful JSON recovery and failure without draft loss. No live gateway was used.
- Durable turn/event log with sequence-based replay, persisted assistant deltas,
  tool results and approval requests. Startup marks in-memory turns interrupted;
  subsequent turns complete interrupted tool-result groups before a model call.
  Failed turns display replayable diagnostics beside the affected chat message,
  including HTTP status or provider SSE error details. Model diagnostics are
  capped at 64 KiB and redact the configured API key before persistence.
- Explicit permission matrix: request_approval/auto_approve/full_trust, global,
  conversation and turn overrides; interactive approval responses verify their
  conversation. Full Trust executes permitted tools without creating a manual
  approval, including Mod receipts, uncataloged downloads and Agent plugin
  publication. It still waits for actual answers to `ask_user` and rejects forbidden tools. Reads and file writes have separate effects; a write outside
  the app data root and registered game roots is not a game write and still
  needs review under `auto_approve`. Plans remain fingerprinted and writes use
  the same durable journal, backups, rollback, and recovery. The desktop
  transaction-history command and local Agent can roll back external file
  transactions in their recorded scope; inbound MCP cannot, even with an ID.
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
  Bing search uses a fresh browser session and keeps raw pages out of that cache.
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
  fallback: approval except in Full Trust, public HTTPS with
  independently validated redirects, disk-space admission and archive signature check.
  The direct-link, signed MCP and completed-download receipt gates recognize
  `.rar` and require a complete RAR4 or RAR5 signature before passing it on.
  The signature check is not a full archive validation.
  Known-length downloads and extracted bytes are admitted against current free
  space with headroom; unannounced response bodies use at most half their starting
  space budget. Cancelled downloads retain only usable, strong-ETag fragments.
  Downloads have connect and read-idle timeouts, not an overall transfer deadline.
  Archive and SHA-256 streaming do not have an overall time limit: they check
  cancellation between chunks and bound the physical input, repeated reads,
  declared and measured expanded bytes, metadata and disk usage. The opaque 7z
  header parser alone keeps a cooperative one-hour guard and a 16 MiB compressed
  metadata read budget. Solid 7z state and CRC verification are not checkpointed
  for restart. ZIP64 is supported, but split or ambiguous ZIP metadata is refused.
  7z uses bounded single-thread decoding
  (4 MiB encoded-header allowance, 128 MiB data-dictionary budget shared by a
  block's LZMA-family decoders); unusual codec stacks and oversized dictionaries
  are not retried without those guards. TAR.xz and sparse TAR remain unsupported.
  The bounded 7z reader requires Rust 1.93 or newer.
  Folder extraction uses the bundled official 7-Zip CLI after that read-only
  preflight. It operates on a private snapshot of the already-open file and
  streams to stdout, never to user-controlled paths. The manager creates safe
  staging entries, checks their sizes and SHA-256 against the preflight, and
  discards the stage on cancellation, a decoder error or a content mismatch.
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
- The read-only `web_search` Agent tool uses the bundled Node and Playwright
  runtime to open Bing and extract ranked titles, destination URLs and snippets.
  Queries use the browser request envelope, and search results can continue across pages. Local paths
  and URLs with embedded credentials are rejected. Returned links must be public
  HTTP(S) URLs. The tool marks excerpts untrusted and bounds serialized results
  to 32 KiB. Missing browser setup reports `needs_configuration`; search failures
  report `unavailable`.
- Unknown-game identification has bounded local name previews, exact-query
  opt-in Bing searches, untrusted candidate selection and independent desktop
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
  The eight opt-in presets are Mod 客栈, Playwright, MarkItDown, mod.io,
  CurseForge, GameBanana, 3DM and Nexus Mods. `system-mod-download` uses the
  connected source MCPs for project and file discovery and download, including
  Mod 客栈. The official Mod 客栈 attachment tool requests a signed link and transfers a selected
  file into managed downloads as one operation under the same approval policy. When a source cannot
  deliver the file, the Agent uses the connected Playwright MCP to download it
  from the official page. Schema v12 gives the stock Playwright browser a
  user-writable output directory and absolute download receipts.
  Schema v9 moves the stock 3DM stdio preset to `https://mod.3dmgame.com/mcp`
  and expires its old launch consent while retaining the credential reference. Desktop bundles carry Node.js/npm/npx and uv/uvx. A confirmed
  bare launcher uses the packaged runtime with a child-only npm or PyPI mirror
  (npmmirror or Tsinghua). Explicit executable paths retain their own configuration.
  Schema v11 updates the six stock npm/PyPI launch commands to fetch current
  releases from the configured package index. Users can still pin a version in
  their own launch arguments. Schema v17 switches the stock Playwright MCP preset to the bundled Node and
  Playwright CLI. Connecting installs the matching Chromium build if needed and
  reuses it from the per-user Playwright cache. Existing stock connections need
  one Start in Settings to authorize the new command. MarkItDown inherits the bundled
  uvx index. Schema v6 removes the redundant index flag and URL from the unedited
  historical preset and expires its old startup consent. Custom configurations are preserved.
  Schema v4 clears prior auto-start consent for bare aliases.
  On 2026-11-21 both Windows x64 release installers (NSIS and MSI) were built and
  their extracted runtime archives matched by SHA-256. Both ran an npm package and a
  Python wheel offline with isolated caches. The native Rust npx smoke also completed
  MCP initialization and tool discovery. The Linux x64 archive passed both package
  tests on WSL Ubuntu. The macOS arm64 archive was inspected but not executed locally.
  `uvx` has a ten-minute startup deadline for dependency installation; other
  stdio processes still have two minutes and HTTP still has twenty seconds.
  On Windows the old PyPI command timed out downloading `pdfminer-six` before
  initialization. The opt-in native Rust smoke with the mirror completed
  initialization and found `convert_to_markdown` in 245 s, then in 11 s on a
  warm-cache repeat. It used a disposable database and did not convert a file.
  A macOS stdio smoke separately exercised Playwright initialization, tool
  discovery and navigation; these are not native settings UI tests.
  Stdio is an explicitly acknowledged unsandboxed OS-user process, not contained
  by FileEngine; stronger cross-platform sandboxing remains required.
- Game adapters can declare optional stdio and Streamable HTTP services in
  `adapter.json` under `mcpServers`. The plugin detail and draft validation show
  the declared services. The MCP settings list identifies their owner and handles
  credentials and explicit startup. Deployment and rollback synchronize the
  service definitions with the verified package inside the registry transaction.
  Disabling the owner or replacing a launch configuration revokes launch consent.
  See [the package example](../README.md#mcp-services-in-game-plugins).
- The 3DM public HTTP endpoint accepts anonymous tool discovery and takes the raw API key in `Authorization` for authenticated requests. The desktop client selects that format by endpoint, including for saved and custom connections. HTTP 401 and 403 startup errors identify the credential or permission problem without exposing response bodies. The opt-in native smoke registered 16 tools on 2026-11-21.
- External MCP connections show an optional single-line description beside the
  service name. Hover or keyboard focus reveals the full text. Built-in services
  include Chinese descriptions, custom connections can be edited in Settings,
  and adapters can supply a `description` per service in `mcpServers`.
  The per-service credentials dialog handles Bearer and stdio environment secrets.
  Each password field shows whether a nonblank value is saved locally, or whether
  its status could not be read. The desktop status command returns booleans for
  the selected connection's references only; it never returns credential values
  or validates them with the provider. Its footer has 返回 and 保存.
  保存 submits the nonblank fields together. Each connection can store an
  optional `credential_url`. The 获取 Key link opens that page in the system
  browser. The credential-backed Mod service presets include their key-management pages.
  Each row shows 未启动,
  启动中, 运行中 (tool count) or 启动失败 with the reason, and has one start/stop
  button. Clicking Start launches the selected external service directly and saves `enabled`.
  The desktop has no fixed limit on the number of connected MCP servers.
  On desktop launch, previously started connections restore in the background.
  Only a successful manual handshake and tool registration save consent to that
  exact configuration selected with Start.
  Merely enabled or legacy configurations have no consent. Stopping, changing
  connection parameters or removing the service
  revokes it. A transient restore failure permits retry on the next launch or
  cancellation from Settings even when disconnected. The local server remembers
  its opt-in separately and recreates its loopback endpoint and token.
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
- The plugin center, library and Agent can discover anonymous Mod客栈 catalog releases. The desktop card’s **安装** action validates the selected immutable release and installs its two files in one journaled transaction, without importing a draft. A matching installed slug is shown instead of offering to overwrite it. **已安装 → 移除** removes an unbound package and its Skill/tool registrations while retaining a recoverable backup. Neither action changes an installed Mod. Agent imports retain the reviewed draft workflow, and library binding remains explicit and checks the game’s markers. The cloud catalog shows releases from different games side by side, with the game cover on the left. The grid switches to one column in narrow windows and the library picker.
- Each desktop launch checks installed catalog adapters in the background after successful transaction recovery. New catalog installations and unedited catalog imports track the originating plugin ID. Earlier direct catalog installs match a unique catalog slug. Only strictly newer versions are applied. Each adapter update is a journaled transaction that preserves game bindings and the user's adapter and Skill settings. Local drafts, modified installations and unrelated packages are skipped. A failed download leaves the installed version available and the next launch checks again.
- Publishing from a draft or installed external plugin generates a review with a model-written title, summary, description and supplementary tags. The source manifest supplies the complete game name as the first publication tag and the original game cover. The authenticated author's complete plugin list determines first release versus update by exact slug. The final click sends the reviewed release copies; the local adapter and installation rules stay intact. Missing read permission, ambiguous ownership or a changed target blocks the write. The public catalog reads the validated game cover URL from the release manifest; missing images have a placeholder, and old pixel art is not displayed. A valid receipt or automatic read-back leads directly to **发布成功**; reopening an interrupted manager-plugin record checks it without sending another publication. See [the publication contract](plugin-publication.md).
- Detailed publication descriptions accept Markdown without a separate character cap. They are preserved in the published Skill and shown as inert text in desktop previews. The existing whole-file size limit for cloud catalog reads still applies.
- Already-published adapters update the current creator's plugin ID, not a historical slug match. Receipts for that ID prevent version reuse after a cloud rollback. A separately approved, strictly newer release can proceed past an older uncertain write when the verified cloud version has caught up; the old record is not replayed or marked successful.
- The installed adapter list and details show when the current version was deployed locally; refreshing or toggling a package does not change that time. The draft list keeps creation time separate from the latest saved change to `adapter.json` or `SKILL.md`. Both lists use local time and show a dash when no reliable timestamp is available.
- The installed adapter list and details show **云端插件** for a confirmed local publication receipt or an exact public catalog slug match. Successful publication and reconciliation refresh the label without changing installation provenance, version or enabled state.
- The frontend has eight lazy-loaded views (including game detail), light/dark
  themes, Skill management, plugin draft/validate/install/enable controls, and
  publication preview/history. These implemented surfaces are not evidence of
  cross-platform or release acceptance.

## Verification

**Windows release smoke, 2026-11-24.** The production desktop was opened with
its existing data to inspect the library, plugin catalog, history, settings and
themes. Write scenarios used a separate release build of the same source with an
isolated application identifier and temporary game files. The sidebar now exposes
archived conversations, Settings names the install and uninstall Skills in Chinese,
and the file approval preview shows the import description, operation and impact
in Chinese. Steam discovery excludes the manifest IDs for Steamworks Common
Redistributables and Ubisoft Connect Services, including directory-scan fallback.

The native WebView2 flow covered first launch, adapter creation and deployment,
Minecraft registration and Fabric diagnostics. Windows selected a local JAR,
approval installed it, and the installed bytes matched the reviewed archive.
A loopback model exercised streamed tool calls, cancellation and archival. After
relaunch the archived conversation was read-only and the recycled test file was
restored byte-for-byte. SQLite integrity and the six committed file transactions
were checked. The local Fabric 1.21.1 bootstrap also produced real English and
Chinese missing-dependency and version-mismatch logs, plus a successful test Mod
entrypoint before the EULA prompt. The native diagnostic parser replayed all five.
Anonymous live checks downloaded a pinned Modrinth JAR and resolved and downloaded
a Thunderstore release. The integrity checks used the downloaded bytes.
The final native scan of the workstation returned seven candidates from 63 entries,
retaining the games and omitting both platform services. Across three viewport
sizes, Playwright passed 995 cases with four viewport-specific skips. The Rust
library suite passed 1,151 cases with 15 opt-in or helper cases ignored. After
the import wording change, all 14 import tests passed. Rust formatting, all-target
Clippy, the frontend production build, and 19 browser-search tests passed.
The final Windows x64 ZIP was unpacked and all seven files matched the release
build. Installer lifecycle, a signed live upgrade, external publication and paid
model credentials were outside this workstation smoke run.

On Windows with the Mihomo tunnel active, system DNS returned both IPv4 and IPv6
benchmark addresses for Mod Inn and Modrinth. The native downloader recovered the
public addresses via HTTPS DNS, reached the Mod Inn site with verified TLS, and
downloaded the 1,297,415-byte public Modrinth smoke-test archive with its expected
SHA-256. The Mod Inn signed attachment path has separate offline regression coverage.

The Windows desktop client discovered the official Mod 客栈 attachment tool as a
public guest. A loopback MCP and file server exercised an approved ZIP transfer
with one signing request, no forwarded API key and a matching hash. A separate
Playwright MCP test clicked a page link in Edge, observed one archive request,
and verified the reported absolute path and SHA-256. Both transfers used fixtures.

In-place message retry and editing were verified on Windows on 2026-11-21.
The Rust library suite passed 746 tests with 10 ignored. All 63 focused
Playwright cases passed across three viewport sizes, including repeated edits,
late stream events, draft attachments and switching conversations during a retry.
The production frontend build and all-target Clippy check passed.

On 2026-11-21 all six unpinned package presets initialized on Windows and listed
tools using the bundled launchers and configured mirrors. MarkItDown exposed
`convert_to_markdown`. The settings and attachment UI suites passed 81 cases
across three viewport sizes, including direct start, retry and repeated clicks.

On 2026-11-21 the Windows native MCP client connected to the seeded 3DM HTTP
endpoint without credentials and registered 16 tools. Isolated stdio handshakes
listed 12 mod.io tools, 13 CurseForge tools, 9 GameBanana tools and 9 Nexus Mods
tools. These probes did not download or install a Mod. SQLite tests cover fresh
seeding, upgrades from the stock stdio preset and preservation of edited launch
configurations. The 3DM settings credential test passed at all three viewports.

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
byte-exact restoration. Its read-only SQLite checks cover the external adapter
deployment, game file transactions, three completed turns, one cancelled turn
and one archived conversation. JavaScript and PowerShell syntax checks and both embedded C# helper
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
node ../scripts/test-rust.mjs --lib
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
and checks that a fresh installation has an empty adapter registry. `hdiutil verify`
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
  login or authenticated REST request was performed. Live model/provider
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
- Authenticated live Mod客栈 manager-plugin publication and owner-scoped reconciliation still need a creator-authorized acceptance run. Offline tests cover first publication, subsequent and hidden releases, permission errors, uncertain results and the publish → public catalog → byte-exact install round trip. Legacy ordinary-work uploads retain their historical recovery rules.
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
- Complete macOS release acceptance: the verification workflow defines an isolated
  packaged-app launch smoke test for fresh SQLite/plugin initialization. A
  temporary copy from the DMG booted locally. Gatekeeper rejected the ad hoc
  build, so normal Finder install/launch remains pending. The fixture smoke does
  not cover crash/disk-full testing or general production archives.
- Run the Windows installer jobs and verify on a clean Windows 11 desktop.
  Linux ARM64 `.deb`
  and extraction-mode AppImage passed the isolated WebView smoke above; native
  FUSE launch, normal desktop acceptance and Linux x86-64 packages are pending.
  Final icon/signing decisions also remain open.

All requirements in the original approved plan remain in scope.

## Desktop game card actions (2026-11-19)

- Implemented desktop IPC commands `update_game`, `remove_game`,
  `open_game_location`, and `launch_game`. These originally shipped as desktop
  card actions. The November 23 game-library tools also expose edit, remove and
  launch through the Agent and inbound MCP.
- Editing preserves game identity and unrelated metadata. A custom launch path
  is checked on save and launch. The macOS `.app` path resolves its declared
  bundle executable. A valid custom executable takes priority over the numeric
  Steam App ID. Discovery and launcher metadata continue to be inert.
- Removal atomically drops the registration and its user-root mappings and
  detaches conversations. Shared root registrations, history, and disk files
  are retained. Active installations, unfinished reviews, and unrecovered file
  transactions block removal. The removal dialog lists the blocking tasks and
  their paths, including standalone file previews under the registered roots.
  Players can cancel an eligible task there, then confirm removal separately.
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

### 界面动画（2026-11-21）
- 新增 `src/motion.css`（关键帧与 `animate-*` 令牌）和 `src/lib/motion.ts`（`stagger`、`useDidChange`）。页面进入、游戏卡片与表格行依次浮入、发现候选行、空状态与错误提示、封面图加载渐显、侧边栏导航依次滑入与当前项指示条、主题切换颜色渐变与图标旋转、复选框与单选框勾选弹出、Badge 颜色过渡、文件夹展开与收起、引导页步骤切换。
- Agent 对话：消息浮入、空状态图标浮动与光环、处理中三点跳动、会话列表行滑入、运行中会话的脉冲环、工具与思考完成时的勾选弹出（只在状态变化时播放，历史记录不重播）、发送与停止按钮切换弹出。
- 所有效果都通过 `motion-safe:` 或 `prefers-reduced-motion: no-preference` 保护，系统开启减少动画时全部关闭；侧边栏指示条属于状态，减少动画下仍显示。`tests/ui/motion.spec.ts` 覆盖默认播放、减少动画关闭、指示条、导航依次进入、主题渐变清理和 `stagger` 取值。
- 2026-11-21 验证：`yarn typecheck`、`vite build` 与 `motion.spec.ts`（3 个视口 21 个用例）通过。整套 `yarn test:ui` 在并行负载下有偶发超时，相关文件单独重复运行通过；`workflows.spec.ts` 中依赖已隐藏的「历史对话」导航项的 2 个用例在改动前就失败。`yarn build` 当前被他人未提交的 `SettingsPage.tsx` 中未使用的 `SecretField` 阻断（TS6133），与动画无关。

### Resumable discovery and diagnosis

- Installed-game discovery has no fixed depth/directory/entry ceiling for
  explicitly selected trees; `maxDepth`, `maxDirectories`, and `maxEntries`
  remain available to specify intentionally smaller scopes. Automatic discovery
  stays targeted at launcher metadata, known instance paths and the shallow
  common Epic fallback. The desktop library and onboarding page through
  candidates and cancel the read without registering anything.
- Desktop IPC, the desktop Agent and loopback MCP use the same session cursors.
  Diagnostic pages carry absolute line numbers and separate file continuation
  flags. Fabric selections/JARs and Bethesda headers are delivered in batches;
  the BG3 UUID check runs against complete parser-safe XML rather than a
  visible text page. Enabled install observations and failed transaction rows
  are continued instead of capped globally. Temporary indices are deleted with
  their workers; safe file/path checks and per-object parser bounds remain.

  The full-size filesystem stress test is opt-in: `node scripts/test-rust.mjs
  --lib selected_scan_paginates_more_than_old_depth_and_directory_entry_limits
  -- --ignored`. Normal regression tests cover depth, cursor/cancellation,
  filesystem guards and cross-page specialist evidence.
