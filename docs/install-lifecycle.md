# Installation Lifecycle

## Game Mod library and desktop Agent actions

New downloads belong to a game category under the active manager data root's
`Mods` directory. Each file has a stable artifact ID, original filename, size
and SHA-256. A download can be filed under a game name before the game directory
is registered. Choose the local game instance when installing. Existing download
and staging folders are not scanned or migrated into the library on upgrade.

The desktop install action sends the game and artifact IDs to the Agent. The
embedded `system-mod-install` guide starts with `match_mod_install`, which reads
the local adapter, archive listing, README and FOMOD configuration. If those leave
the destination unclear, the Agent reads the author's description via the source
MCP, then the verified project page via Playwright when necessary. Explicit
mappings support individual mods with no installed adapter. Targets resolve under
the selected game's registered directory, or an adapter's registered root.

`install_library_mod` prepares the exact FileEngine plan that its approval commits.
Single-file mods copy from the library's `archive` directory. Other layouts copy
from a verified extraction in the entry's `payload` directory. Reinstall uses the
same library entry. The game detail uninstall action uses `system-mod-uninstall`,
`inspect_install` and the recorded install ID to select trash or restoration of
the original files. Conflicts with shared or edited files are reported. The
library archive is available after uninstall.

In the game’s Mod 管理 tab, 已安装 means the files have been deployed.
已卸载 keeps the install record and any associated library archive available for
reinstallation. 删除 removes the selected mod from this game's management list.
For an active installation, the Agent normally uninstalls or restores overwritten
files, then requests removal of the exact inactive record. If the entire verified
manifest is already absent and contains only exclusive additions, an explicit
delete can instead release ownership of that record without touching game files.
An ordinary uninstall request does not take this shortcut. Archived bytes and the
transaction journal remain available on disk. A never-executed preview can be
cancelled separately without touching a same-name installation. Restoring a
previously uninstalled mod through the journal makes its active record visible again.

Turn on **批量操作** in Mod 管理 to select individual rows or all actionable rows.
Batch install and uninstall skip inapplicable items and do not submit an archive twice.
Each submitted Agent task retains the existing permission, approval and readback flow.
Batch deletion first lists the selected targets for confirmation. Removing one install
record does not hide other visible histories that use the same archive. Turning batch
mode off clears the selection without cancelling already submitted tasks.

## Path-based installation API

`plan_install` accepts a registered game ID, an inert artifact under manager
downloads/staging, its original filename, a declared mod type and display metadata.
An optional expected SHA-256 is verified; a completed-download catalog record also
binds the filename and actual hash. Local hashes never establish authenticity.

The chosen game/user root must still be the exact canonical directory the user
registered. Discovery hints and Agent arguments cannot register a new root.

Single-file rules preserve the payload, including Minecraft JARs and resource-pack
ZIPs. Directory rules retain per-mod manifests. Prefix rules inspect the actual
layout, including at most one outer wrapper. The two desktop install entry points
and Agent can preview a unique FOMOD `ModuleConfig.xml` and turn required or
explicitly selected options into `mappings`. File references resolve relative to
the directory containing `fomod/`, including inside an outer archive wrapper.
Folder references expand into regular descendant files from the verified archive
inventory. Each mapping preserves the full archive entry name and relative
destination, including subdirectories. An explicitly empty folder destination
maps its contents to the selected install root. The preview rejects missing
sources, path escapes and more than 10,000 total expanded mappings across all
options. Both desktop entry points preselect deterministic `SelectAll` and
required single-option groups when
`automaticSelection.status` is `safe`. Option records include the group and
option indexes, so groups with the same name remain distinct. Duplicate sources,
duplicate destinations, file/directory conflicts, or state-dependent rules send
the configuration to manual review with no preselected options. The table reflects
the current selection and its exact source and destination paths.

An Agent can set `modType: "fomod"` and `fomodSelection: "automatic"` in
`install_mod` or `plan_install` when the adapter declares the FOMOD manual
archive rule. Rust computes the mappings from the same hash-verified extraction
used to prepare the plan. The caller can provide `expectedSha256` to bind a
download or a previous preview. The request is rejected if it also supplies
manual `mappings` or if the configuration requires user choice. For configurations
with choices, preview the archive and confirm the intended files before submitting
explicit `mappings`. Conditions, scripts and priority-based conflict resolution
are outside the automatic selection rules. Staging is retained on failure.

The preview lists every target, original fingerprint, overwrite/add impact, ignored
file and warning. Approval then applies that exact FileEngine plan. Any changed
source/destination invalidates it. Install records exist before execution; enabled
state is derived from committed transactions, including after process restart.
If a trashed install is restored in multiple transactions, its state is derived
from the committed restore records and can be reported as `partially_restored`;
it is not presented as a fully restored install until all tracked items return.

AutoApprove remains available for ordinary changes within registered roots.
Download/staging-source plans need review even through generic file tools. The
explicit FullTrust policy can skip review, but never path guards or the prohibition
on deploying partial/quarantined downloads.

`plan_uninstall` moves only the install's tracked, unchanged, exclusively owned
files to trash. It preserves unknown/configuration files. If the installation
overwrote previous files, use its transaction rollback so originals are restored;
merely moving the replacement away would break the game. Trash restoration is
itself a reviewed plan. Permanent purge exists only behind the desktop's explicit
confirmation and preserves database tombstones, not a silent Agent operation.

## Exact Download Origin

Each finalized download has its own immutable catalog entry and file ID, even
when it was retrieved from the same URL as an earlier download. The resume
fingerprint still identifies the request, not the completed file. The catalog
does not store the download URL.

When planning an install from a bound catalog entry, the canonical path,
original filename and current SHA-256 must match. The preview and install
manifest record `downloadOrigin` (`artifactId`, `sourceId`, `projectId`,
`versionId`) before execution. The display version is not the version ID.
Callers can omit source, project and display-version hints; conflicting hints
are rejected instead of relabeling the artifact. The file plan still needs its
normal approval and the usual source/destination fingerprint checks at apply.

Game Details, Agent and MCP expose the same recorded origin. Local payloads
and legacy fingerprint-keyed downloads remain installable; older installs
remain listable. None acquire a guessed origin. A bound origin describes what
was deployed, not remote authorship, current file contents or loader activation.

## Reviewed Version Replacement

An explicit `replaceInstallId` on `plan_install` or `install_mod` replaces a
deployed version with a chosen catalog version, not necessarily a newer one.
The source and project IDs must match, the version IDs must differ, and the
registered game, Mod type and installation root must agree. Local or legacy
installs without exact origin cannot be replaced this way.

One preview shows the files added, the same-path files backed up and
overwritten, and the old-only files moved to trash. Existing files not owned
by the original installation are not overwritten. Replacement refuses edits
to tracked files, paths shared with other installs, overlapping or case-aliased
paths, and an original installation that overwrote pre-existing content.
State and ownership are checked again under the write lock before applying;
unrelated files such as user configuration remain untouched.

The SQLite lineage is saved with the plan before approval. A committed
replacement marks the old record `superseded`; rollback restores the old
record and reverts the full journal. Multiple replacements must roll back in
reverse order. Missing backups, changed files, newly occupied paths and
shared ownership block rollback before it changes any files. Retired payloads
are visible in trash, but cannot be restored individually while the replacement
is active. The desktop's explicitly confirmed purge can make rollback unavailable.

This workflow does not pick an update release, verify compatibility, solve
dependencies, merge shared file contents, or launch the game.

## Reconciliation Preview

`inspect_install` is a read-only coordination pass shared by desktop IPC, Agent
and local MCP. In addition to current file fingerprints, it reports a
`reconciliation` object with a conservative `recommendedAction`:
`safe_uninstall`, `rollback_install`, `rollback_replacement`,
`rollback_successor`, `forget_missing_files`, `review_shared_paths`, `review_file_state`, or
`review_install_state`. A rollback recommendation is emitted only when the
journal contains a committed replacement link or install transaction that can
be identified without guessing.

`blockingInstallIds`, `successorInstallId` and `rollbackTransactionId` are
evidence for the next user-visible action, not an approval. The Agent must still
call the recommended operation; ownership, path, backup and
current-fingerprint checks run again immediately before mutation. Ordinary
shared installs remain manual-review cases, and this preview never merges bytes,
rewrites manifests, or changes database/file state.

`forget_missing_files` is not a rollback recommendation. On an explicit removal,
`forget_uninstalled_mod` rechecks the exact game, install, and missing-file evidence
under the writer lock. It records a `removed_missing` disposition and hides the
list entry atomically. The original install transaction stays committed; no
uninstall, rollback or trash receipt is fabricated.

## Modrinth Dependency Review

Use `resolve_mod` with the download's exact upstream project and version IDs to
look up direct declarations. Pass those declarations and the registered game ID
to `check_modrinth_dependencies` via Agent, local MCP, or desktop IPC. This
read-only check compares the declarations with exact catalog origins in the
selected game's install journal. It distinguishes missing declarations (`null`)
from an explicit empty list.

A pinned match, a project-only match, another recorded version, and no recorded
match are distinct outcomes. File-name-only references, multiple active records,
inconsistent IDs and partially restored installs remain unconfirmed. An embedded
declaration describes upstream metadata, not a verified file. A selected
replacement excludes only that record from the comparison, rather than counting
the version about to be removed; this is not a simulation of the new install.

Results are advisory. They do not account for manually deployed files, legacy
records without catalog origins, other providers, present-day file integrity or
whether the loader actually activated a Mod. No dependency is fetched or deployed
as a side effect, and the check grants no file approval.

The desktop IPC `review_modrinth_dependency_graph` takes a completed Modrinth
download, a registered Minecraft Java target, and its game version and loader.
It resolves the root's exact project and version from the local download catalog
and compares each graph edge with the game's installed records. For a selected project-only dependency, the comparison uses the
resolved exact version, not merely any installed version of the project. A
candidate or an exact ID entered by the user is revalidated against the provider.
A chosen replacement is excluded only if it remains an enabled record for a
different version of the root project in that game. For each selected required
dependency with one exact enabled installation record, the same review checks
the current files against that record's sizes and SHA-256 hashes. It checks
repeated references to the same install only once per review. Evidence marks
matching bytes, changes, omissions, and unverified entries separately from the
journal result. It reports up to four representative relative paths per install;
there are no file contents in the response or requests to Modrinth.

File reads use the selected game's registered roots and the install manifest's
recorded destination. Changed paths, links, unavailable files and legacy records
without a valid manifest cannot produce a complete match. The local pass limits
each manifest to 512 KiB and shares a budget of 256 entries and 512 MiB of hashes
across matched installs. An eight-second soft deadline stops scheduling more
reads; it cannot interrupt a blocked OS call. The manager holds its own writer
lock during this local pass, but external programs can still change files.
This is observation, not proof of loader activation, authorship, or an archive's
safety. Creating an installation plan is still a separate action.

Agent and local MCP clients can separately use
[`resolve_modrinth_dependency_graph`](mod-sources.md#transitive-dependency-metadata)
before installation to inspect required transitive declarations. A project-only
choice, an unavailable declaration, a cycle, or a catalog conflict should be
shown to the user instead of being treated as a successful install. The local
journal comparison and any subsequent download or file approval are separate.
For a managed installation, the read-only `verify_install_files` Agent and
local MCP tool accepts `game_id` and `install_id` from `list_installs`. It checks
the same local size and SHA-256 evidence for any source, only for an enabled
record in that game. The tool does not access a provider or return file bytes,
and it cannot generate or approve a repair plan.

## Native Smoke Evidence

On 2026-11-18, the unsigned macOS arm64 release app was launched and exercised
with an inert local archive in the already-registered
`output/native-smoke/minecraft` fixture. The native file picker selected the
archive; import approval staged it under transaction
`txn_04a8bf623fe5494eb6b232e98b335f39`. A separate installation approval used
transaction `txn_15c539fddfce439d945708ccca2b5762`; game detail then showed one
installed and one enabled item. This was a local fixture smoke test, not a real
mod download, external publication, or game-launch test. The inert JAR's
SHA-256 was `bc0ec11c0a3c912ea0e615a5f5197156376efca2027fa54a30352bc501caad36`
before import and remained identical in staging and after installation.

Remaining limitations: automatic update selection, dependency/version solving,
condition-aware FOMOD selection that requires game/flag state, automatic shared-file
repair/merging, and
descriptor-based protection from malicious concurrent path replacement.
Installation success means the approved files were deployed, not that the game
was launched. Release and cross-platform acceptance are tracked in
[implementation status](implementation-status.md).
