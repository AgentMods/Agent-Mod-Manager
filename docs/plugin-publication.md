# Manager plugin publication

## Public catalog

The desktop plugin center, library, and local Agent use the public Mod客栈 manager plugin catalog. The Rust client reads `GET /api/v1/manager-plugins/games` anonymously. Selecting an immutable release loads `adapter.json` and `SKILL.md` from `/api/v1/manager-plugins/releases/{releaseId}/` on the same host. In the desktop catalog, **安装** authorizes that release only. The backend checks its identity, validates both files, and installs without creating a draft or replacing an existing plugin. **查看详情** optionally shows the original texts and declared MCP services. From **已安装**, **移除** removes the unbound local package, not the cloud release or any game Mods. Agent imports and manual drafts keep their review workflow; applying an installed adapter to a game remains a separate explicit action.

## Publish a manager plugin

In **已安装**, the list and detail panel show **云端插件** when a confirmed manager-plugin publication receipt or an exact plugin slug in the public catalog establishes cloud presence. A published adapter keeps this label even if its installed version differs from the cloud version. The label does not alter the local installation source or grant update permission. When the catalog is offline, local receipts still work and unconfirmed publications alone do not change the label.

Select **发布插件** on a committed draft or a verified installed package. Preparation reads the current creator's plugin list and asks the configured Agent model to generate a title, short introduction, detailed description and tags. The preview uses the same validated `adapter.json.coverUrl` as the installed game card, not model-generated artwork. A missing or unavailable image is shown as a placeholder and does not block publication. The preview shows the exact publication version and whether the current account will create a plugin or update an existing one. The **发布** button submits the reviewed release. In Agent chat, the same prepared release is submitted without a manual approval in Full Trust; other modes show the publication review.

Configure an approved creator's Mod客栈 key with both `manager_plugins.read` (match the account's own list) and `manager_plugins.publish` (submit the release). Matching uses the package slug across the author's paginated list, including hidden plugins. A failed or ambiguous lookup leaves the publication unsent. The public catalog still works anonymously.

| Content | Stored in the cloud release |
| --- | --- |
| Title and version | `name` and `version` in the published copy of `adapter.json` |
| Short introduction | `description` in the `SKILL.md` frontmatter |
| Full description | A display section in `SKILL.md` |
| Game cover | The original `coverUrl` in `adapter.json`, unchanged; no image is generated or uploaded |
| Tags | The first tag is the complete game name from the source `adapter.json`. Other tags come from the generated copy. The reviewed list is sent unchanged as the manager-plugin MCP `tags` parameter. |

Publishing sends the prepared copies with `publish_manager_plugin` or `publish_manager_plugin_release`. Local source and installation rules are preserved. The desktop cloud catalog can retrieve the artwork and detailed description by immutable release ID. The website's own listing controls how much of that content it displays. Newer releases of a hidden plugin keep their moderation status. A matching receipt shows **发布成功** immediately. If the write reply is inconclusive, the app automatically checks the remote release without repeating the write. Opening an interrupted record also checks it automatically; **刷新发布状态** is available if confirmation is still unavailable.

For an existing plugin, the preview chooses a version above both the current remote version and any higher version already confirmed locally for that remote plugin ID. The publish step rechecks ownership and the exact remote release shown in the preview. It never changes an approved first publication into an update or silently retries a rejected write. An in-flight send still blocks competing writes. An uncertain record is never resent or marked successful based on version alone: a separately reviewed update can proceed only if its verified cloud head has already reached or passed the uncertain version for that plugin and its new version is strictly higher. Recovery requires matching owner metadata and the approved release texts. Legacy uncertain records without a credential binding still require reconciliation.

Old pixel-art previews must be prepared again before sending. Existing cloud releases and local history are not rewritten. The automatic path validates that its final copies fit the desktop catalog read limits (256 KiB for the manifest and 128 KiB for the Skill). The current website v1 parser does not accept `mcpServers`; a package with non-empty service declarations is reported as unsupported rather than silently publishing it without its tools.

## Earlier ordinary-work publication records

Earlier versions published ZIP attachments as ordinary works. Their stored approvals, hashes, IDs and events belong to that workflow. Existing records can be inspected and eligible uncertain states can be reconciled. All new publications use the manager-plugin tools above. The remaining sections describe the historical format for maintenance and recovery.

## Scope and Trust

`src-tauri/src/plugin_publish.rs` implements local preparation, durable consent
and a publication state machine. It does not make network requests, manage MCP
presets, read API keys, execute package contents or report remote success without
a transporter supplying a verified result. The remote implementation lives in
`plugin_publish_remote.rs` and is owned separately.

The destination is fixed to `https://www.glossmod.com/api/v1/mcp`. There is no
custom endpoint, arbitrary local path, arbitrary file list or remote response
JSON parameter. The public DTOs use PascalCase Rust names and camelCase JSON.

Publication is permitted for either:

- A valid draft whose original, reviewed write transaction is committed.
- An installed, validated external package at its derived managed location.
  Its files must match a committed
  deployment, its recorded raw content, both file fingerprints and the reviewed
  draft/write/deployment paths. A registry boolean alone is insufficient.

Schema validation reuses the package parser and draft provenance validator.
Sources must contain exactly `adapter.json` and `SKILL.md`; links, redirected
parents, extra files, special files and invalid UTF-8 fail validation. Local file
reads use the authorized transaction and the available resources. On Unix,
opened file device/inode identities are also checked. The manager file lock
serializes reads against its own transactions. This is not an OS sandbox
against another process changing the manager's database or files; trusted
transaction fingerprints and fresh content comparisons remain mandatory.

## Preview and Bytes

```rust
prepare_package(&FileEngine, draft_id: &str, PublicationMetadata)
    -> AppResult<(PublicationPreview, PackageBytes)>
prepare_installed_package(&FileEngine, slug: &str, PublicationMetadata)
    -> AppResult<(PublicationPreview, PackageBytes)>
```

Preparation only reads source files/database records. It creates no scratch
files, ZIP file, registry changes or database tables. `PackageBytes` wraps private
ZIP bytes and deliberately implements neither `Serialize` nor `Debug`; only
`as_slice`, `into_vec`, `len` and `is_empty` expose them to trusted Rust code.

`PublicationPreview` contains `package`, `source` and `digest`. Its `PublicPackage`
includes all exact uploaded file names, unmodified UTF-8 text, byte sizes and
SHA-256 values, plus slug/version, draft ID (null for installed packages), title,
summary, description, tags and game IDs. It also discloses the deterministic ZIP
name, SHA-256 and size, `application/zip`, the fixed MCP destination and the
intended work status `published`. Do not truncate or hide file text or metadata
in the user confirmation view. Raw Skill instructions remain untrusted text.

The ZIP is constructed entirely in memory using Stored compression, the fixed
default ZIP timestamp, fixed Unix permissions 0644, and exactly these ordered,
relative entries: `adapter.json`, `SKILL.md`. Repeated
preparation with identical source and metadata yields identical bytes and hash.

Metadata is not silently trimmed, rewritten or deduplicated:

- Title: 2 through 160 Unicode characters, at least two non-padding characters,
  no control characters.
- Summary: at most 500 characters; description: at most 50,000 characters.
  Embedded NULs are rejected.
- Tags: at most five distinct, nonempty, unpadded values of at most 20 characters,
  without control characters.
- Game IDs: at most ten distinct positive signed 64-bit integers.

The consent digest is SHA-256 over a versioned domain separator and canonical
struct serialization of both source provenance and the complete package.
Database reads rebuild the package/ZIP and recompute this digest, checking
destination, content type, work status, archive name/hash/size, file text and
metadata. Comparing two stored copies of a digest is not sufficient. Artifact
access and each mutating network-step start additionally rebuild from the live
source. Reconciling an earlier effect uses the immutable stored preview, even
if the user has since changed the draft.

## Persistent Consent

```rust
migrate(&rusqlite::Connection) -> AppResult<()>
create_publication(&FileEngine, &PublicationPreview) -> AppResult<PublicationRecord>
approve_publication(&FileEngine, id: &str, approved_digest: &str)
    -> AppResult<PublicationRecord>
get_publication(&FileEngine, id: &str) -> AppResult<PublicationRecord>
list_publications(&FileEngine) -> AppResult<Vec<PublicationRecord>>
verified_upload_artifact(&FileEngine, id: &str, approved_digest: &str)
    -> AppResult<(PublicationRecord, PackageBytes)>
```

The additive, repeatable migration creates `plugin_publications` and
`plugin_publication_events`. The application integrates it into database
startup; pure preview never runs migrations. Publication IDs are stable
`publish_<32 lowercase UUID hex characters>` and are suitable as `create_upload`
request IDs. An exact preview digest has a unique record: repeated preparation
returns the existing ID, including unknown or terminal records after restart.
Changed metadata/content/provenance creates a new unapproved ID.

Records retain the complete immutable preview, approved digest, state, phase,
last step, integer remote file/work IDs and timestamps. Every transition has an
append-only event written in the same SQLite transaction. There is no delete,
reset, blanket retry, preview replacement or remote-response-storage API.
History listing returns at most the latest 100 records; each JSON read is capped
at 3 MiB. Older records remain available by ID and are not deleted.

`approve_publication` must be called only by the trusted user/Agent approval
handler after displaying the full preview and checking the exact approved
digest. A digest binds content; it is not an authentication credential and an
agent must not be allowed to self-approve. Approval does not authorize changed
metadata, destination or source. Staleness requires another preview and consent.

## Transport Contract

```rust
begin_step(&FileEngine, id: &str, digest: &str, PublicationStep)
finish_step(&FileEngine, id: &str, PublicationStep,
            file_id: Option<i64>, work_id: Option<i64>)
await_scan(&FileEngine, id: &str)
mark_unknown(&FileEngine, id: &str, PublicationStep)
fail_step(&FileEngine, id: &str, PublicationStep)
// All above return AppResult<PublicationRecord>.
recover_pending_publications(&FileEngine) -> AppResult<Vec<PublicationRecord>>
```

`PublicationStep` is `CreateUpload`, `StartUpload`, `PutPart`, `CompleteUpload`,
or `PublishWork`. The JSON phases, in order, are:

```text
prepared
  -> createUploadPending -> uploadCreated
  -> startUploadPending  -> uploadStarted
  -> partPutPending      -> partUploaded
  -> completeUploadPending -> awaitingScan (when needed) -> uploadReady
  -> publishWorkPending  -> completed
```

The parallel state is `intended` while ready for a step, `pending` before a
side-effecting network call, `unknown` for uncertain outcomes, `completed` only
after confirmed publication, or terminal `error` for a definitive failure.

The transporter must durably `begin_step` **before** sending each side effect.
Only the allowed preceding phase can begin; repeated pending/unknown/terminal
starts are errors, not retry permission. SQLite immediate transactions protect
this claim across independent database connections. The local module stores no
network body or API key and never sends a request itself.

Use `finish_step` only after validating the corresponding remote response.
Create-upload requires a positive file ID. Subsequent file IDs cannot change;
publish-work requires a positive work ID. Matching successful finishes are
idempotent. Unknown can be resolved by a successful finish only after obtaining
independent evidence, never by resending the uncertain operation.

An explicit uploading/scanning response from complete-upload can be recorded
with `await_scan`. It is safe to poll the provider's read-only upload-status
tool, then call `finish_step(CompleteUpload, ...)` only after readiness, size,
SHA-256, identity and safety checks pass. Do not repeat the completion POST to
poll. No `PublishWork` step can begin until the local phase is `uploadReady`.

Timeouts, lost replies, cancellation after sending, malformed responses or a
crash between send and durable finish must retain `unknown`, with the pending
phase/last step and known IDs intact. On application startup,
`recover_pending_publications` marks **all** persisted pending actions unknown
atomically and never retries them. Do not run this recovery against live active
transports. `fail_step` is only for a definitive failed pending action; it
cannot clear an unknown effect. In particular, `publish_work` is non-idempotent
and must never be automatically replayed.

Credentials, signed upload URLs, authorization headers and remote error bodies
must stay outside these records. The APIs only accept integer remote IDs, not
arbitrary response strings/JSON. Do not add response-body logging as an error
shortcut. Package raw text and metadata intentionally persist because they are
the exact content explicitly proposed for publication; users must review them
for secrets before approving.

## Read-Only Reconciliation

The desktop-only `reconcile_plugin_publication({ id })` returns `verified` or
`unconfirmed` (or a redacted error). It may update the local record and event
history, but cannot change remote state. Only records waiting for a scan or
with an uncertain `CompleteUpload` or `PublishWork` and a known file ID are
eligible. Uncertain upload creation/start/part transfers still require manual
inspection; byte counts by themselves do not attest to the uploaded content.

The provider's unauthenticated `tools/list` on November 18, 2026 documented
`get_upload_status(file_id)` as restricted to caller-owned files, and
`list_works(scope="mine")` as the caller's own work shelf with attachment IDs.
Reconciliation first checks file identity, ZIP size and SHA-256, readiness and
positive scan safety. For an uncertain publish it additionally searches the
account's published works by the previously approved title, capped at 20 rows.
It requires a unique work referencing that file with the exact title, summary,
Mod version and a single matching public latest-version attachment. Zero rows,
a full page, unavailable ownership evidence, or a mismatch cannot authorize a
retry. Only a still-matching local record can advance, checked inside the same
SQLite transaction as its event write. The UI offers no write while waiting
for this check; a newly publishable record needs fresh explicit confirmation.

## Verification Status

The earlier desktop IPC exposed `prepare_plugin_publication`, `list_plugin_publications`,
`reconcile_plugin_publication`, and `publish_plugin`. Preparation is local only.
The publish IPC requires `PUBLISH_PLUGIN` and the exact preview digest; it uses
the same injected encrypted credential store as the Agent runtime. The UI shows
both raw files, every metadata field, the fixed destination, ZIP size/hash and
preview digest. A checkbox and exact confirmation text are required for each
upload/continue action; closing and reopening clears that confirmation. Pending,
unknown and completed records do not offer an automatic resend button.

The Agent `publish_plugin` tool declares `Effect::Publish`: default and
auto-approve modes route its full preview through the local approval broker.
Full-trust mode follows the explicitly selected permission policy. Neither a
separate self-approval tool nor unrestricted remote-publication IPC is exported.
Cancellation and changed source bytes are checked before credential/network
access. The final tool result contains bounded state/IDs/hashes, not a second
copy of the raw package text.

The production transport uses the observed MCP contracts `create_upload`,
`start_upload`, `get_upload_status`, `complete_upload`, and `publish_work`.
ZIP bytes are sent with PUT only to the exact same-origin
`/api/v1/uploads/<file_id>/multipart/1` path. Redirects, HTTP retries, automatic
session reinitialization and arbitrary remote tools/paths are disabled. Received
bytes, filename/type/size, file ID and finalized SHA-256 are checked. The positive
safety predicate follows the provider's public download-safety helper; scan
status fields are strings, not an invented enum contract. Safety and identity
are read again before publication, and successful work metadata/attachment IDs
must match the approved package. Application logs suppress RMCP/HTTP-library
payload logging before credential redaction can be bypassed.

Focused local tests cover deterministic ZIP entries/bytes/hash, metadata bounds,
committed provenance, stale files, symlinks, installed-package verification,
approval binding, tamper detection, persistent history, bounded listing,
restart recovery, phase ordering, idempotency and the absence of remote-secret
storage fields. The 16 preparation/state tests perform no network requests.
Twenty-four transport tests use in-memory mocks and local HTTP only, including
exact ZIP/metadata, ordered durable transitions, consent, interrupted writes,
readiness/safety gates, no duplicate sends, no redirects, reconciliation across
restarts, ambiguous/mismatched evidence, and stale-record rejection. Agent
tests cover rejected approvals in both review modes, cancellation, stale source
bytes and invalid arguments. Eighteen Playwright cases cover the publication UI
across three viewport widths; their IPC is mocked, not a live upload.

**An authenticated end-to-end publication against the actual provider has not
been completed.** The complete provider authorization/scan lifecycle still
requires acceptance with an explicitly authorized API key. Without explicit
positive safety evidence the transport stays in awaiting-scan or rejects the
response. It never treats local preparation or upload as a published work.
Unknown outcomes without positive evidence still require manual provider
inspection. This document does not certify a live upload/publish round trip.

## Status Dialog Prototypes

The light and dark prototypes were generated with `custom-imagegen`, provider
`model_providers.custom`, model `gpt-image-2.5`, and inspected before editing
the dialog. Local outputs are under `output/imagegen/agent-mod-manager-prototypes/`
as `publication-status-light.png` and `publication-status-dark.png`.

Prompt (dark variant changes `Light theme` to `Dark theme` and `flat white`
to `flat charcoal`):

> Light theme. Shippable Chinese desktop utility UI mockup for Agent Mod Manager, centered existing publication review dialog over muted 插件中心 / 发布记录 data table. Minimal crisp modern desktop UI, flat white surfaces, neutral borders, green accent only for verified, amber badge for uncertainty, no decoration. In modal: title 确认公开发布适配包; package name, version, destination Mod 客栈, and state row 结果不确定. Directly below metadata place a compact, inline unconfirmed result with icon and copy 尚未核实，发布状态保持不变, then expandable adapter.json and SKILL.md file previews. Bottom-right action group 关闭 then outlined refresh-icon button 核对远端状态. No publish button in this state. Quiet, compact layout, coherent Chinese typography, no marketing, no watermark.
