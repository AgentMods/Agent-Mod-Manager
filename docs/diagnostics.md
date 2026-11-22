# Game Diagnosis

Diagnosis is read-only. Desktop, Agent and local MCP use the same bounded
reader and check journal-derived install state without refreshing stored status.
Only registered roots and adapter-declared sources are inspected. The tool
never runs a game, probes the live game process, rewrites a profile or applies a repair.

## Text And Evidence

UTF-8 (with or without BOM) and BOM-tagged UTF-16 LE/BE are decoded strictly.
An unrecognized encoding, malformed input, or binary data is reported as
unreadable, not treated as an empty successful read. An explicit
`legacyLogEncoding` in the diagnosis/read options applies only if a log lacks a
BOM and is not valid UTF-8. Supported options are `windows-1252`,
`windows-1251`, `gbk`, `big5`, and `shift_jis`. The default does not guess a log's
code page. Skyrim SE and Fallout 4 plugin lists separately use Windows-1252 as
their legacy-format fallback, based on the libloadorder asterisk format
implementation; the log setting does not affect these lists.

`read_logs` and `diagnose_game` return bounded pages. For `read_logs` use
`nextCursor`; for `diagnose_game` use `logs.nextCursor`. Pass the cursor as
`options.page.cursor` in the next call with the same game and client
conversation. Keep reading until the cursor is null; an intermediate page can
be empty when only directory entries or a partial physical line were processed.
Use `options.page.cancel: true` with the cursor (or a caller-generated UUID in
`options.page.requestId`) to stop. Cursors are in-memory, are not persistent
snapshots and expire if left waiting for another page. Local MCP must keep the
same `conversation_id` when continuing.

File count, raw bytes per file/page, and line count bound the page, not the
investigation. Positive caller options can reduce batch sizes; larger requested
ranges are consumed across batches. LF, CRLF and CR count as physical line
breaks even across buffer boundaries, and line numbers are absolute. An
oversized individual line is kept as a display-only excerpt and does not become
matching evidence. The tool does not return text from a malformed or binary
batch; already delivered valid batches are not silently rewritten.

Each file read verifies the regular, single-link object against its authorized
path, size and modification time. Iteration doesn't follow symlinks or expand
scope beyond the active adapter's roots; a change in registration requires a
new run. This is not a snapshot of the directory or a running process.

## Minecraft Java: Fabric Startup Log

When the Minecraft Java adapter's `logs/latest.log` is included in the read
report, `diagnose_game` also returns `fabricStartup`. It reports recognized
Minecraft and Fabric versions, the startup line, and either the resolver's
reported selection or its explicit failure. Mod rows carry IDs, version text,
physical line numbers, and a marker for nested entries. The UI distinguishes
partial, unavailable, unrecognized, and ambiguous evidence. It doesn't merge
multiple observed starts or use a same-named crash report as the instance log.
The selection parser follows contiguous rows across raw log pages. Recognized
selections are returned in batches of at most 64 rows; the full validated
selection is used for disk correlation. An incomplete section, an unsupported
record, or repeated IDs is not a complete selection.

A selection report describes what the loader chose at that point in a prior
startup. Fabric emits the list after resolving candidates and before invoking
entry points; a game can still fail afterward. A Mod listed here might be
manually installed, but the list does not establish its path, ownership, exact
Modrinth release, SHA-256, initialization or current process state. Likewise,
missing or clipped records do not prove that Fabric or a Mod is absent. Log
mtime is a file timestamp, not proof of when a particular game session started.
Only recognized structured text is interpreted; other loaders, launcher-only
logs, differing log formats, and inference of actual initialization remain
unsupported.

### Resolver Failures

A recognized failure can produce a specific `fabric_dependency` observation
with the requiring Mod ID/version, dependency ID, and original physical line.
The line preserves Fabric's reported requirement and, for a mismatch, the
version or versions Fabric reported finding. The short `Reason` summary is
not used to distinguish missing dependencies from wrong versions.

The parser requires a contiguous main-thread resolver sequence, the standard
exception marker, all dependency rows, and the transition to the loader stack
frames. Supported detail languages are English and Simplified Chinese (`zh-CN`),
recognized from the log rather than the display language. Exact versions,
one-sided bounds, a single bounded interval, and major/minor wildcards are
recognized without re-solving the requirement. The parser considers at most 64
lines after the failure marker and 16 details. Mixed or unsupported languages,
alternative ranges, conflicts, optional recommendations, unsupported rows,
interleaved entries, or a clipped detail block leave the generic failure
observation rather than a guessed dependency.
Neither the evidence nor Fabric's suggested fix is a package identifier,
download URL, diagnosis of the current filesystem, or authorized repair.

The grammar is checked against Fabric Loader `0.16.14` source:

- https://github.com/FabricMC/fabric-loader/blob/0.16.14/src/main/java/net/fabricmc/loader/impl/launch/knot/Knot.java#L128-L164
- https://github.com/FabricMC/fabric-loader/blob/0.16.14/src/main/java/net/fabricmc/loader/impl/FabricLoaderImpl.java#L230-L233
- https://github.com/FabricMC/fabric-loader/blob/0.16.14/src/main/java/net/fabricmc/loader/impl/FabricLoaderImpl.java#L300-L350
- https://github.com/FabricMC/fabric-loader/blob/0.16.14/src/main/java/net/fabricmc/loader/impl/discovery/ModResolver.java#L126-L135
- https://github.com/FabricMC/fabric-loader/blob/0.16.14/src/main/java/net/fabricmc/loader/impl/discovery/ResultAnalyzer.java
- https://github.com/FabricMC/fabric-loader/blob/0.16.14/src/main/resources/net/fabricmc/loader/Messages.properties
- https://github.com/FabricMC/fabric-loader/blob/0.16.14/src/main/resources/net/fabricmc/loader/Messages_zh_CN.properties

### Native Smoke

With Node, `curl`, JDK 21 (set `JAVA_HOME` as needed), and network access:

```sh
node scripts/run-fabric-resolver-smoke.mjs
AMM_FABRIC_SMOKE_DIR=/path/printed/by/script \
  node scripts/test-rust.mjs --lib fabric_replays_real_resolver_smoke -- --ignored
```

The first command downloads checksum-pinned public Minecraft 1.21.1 and Fabric
Loader 0.16.14 artifacts and dependencies, builds a local fixture Mod, and
starts five isolated dedicated-server bootstraps. The missing and older
dependency cases run in both `en-US` and `zh-CN` and fail before the fixture
entrypoint. The baseline selects the fixture, invokes its entrypoint, then
stops at the unaccepted EULA.
The script leaves all five native `logs/latest.log` files and console captures
in a fresh temporary directory. The second command explicitly copies only
the native logs into registered test instances and runs the read-only
diagnosis; it does not use console output as a substitute for `latest.log`.
This does not validate a running server, a client, or third-party Mod activation.

### Current JARs

`diagnose_game` also reports `fabricDisk` for the Minecraft adapter's exact
instance `mods/` declaration, even if its startup log is missing. It considers
non-dot, direct-child `*.jar` names only. It does not recurse, open disabled
JARs, follow links, extract files, load classes, or contact a mod catalog.
It reads only the exact root `fabric.mod.json` in a supported JAR. A ZIP32
preflight bounds archive indexing to 1 MiB and rejects duplicate exact
manifest names and unsupported layouts. Metadata is limited to 64 KiB, with a
100:1 expansion bound and checked sizes and CRC. Only schema version 1 and
the documented subset of Mod ID and version strings supported by the log
parser are compared. ZIP64 and unsupported manifests are not absence evidence.
The Fabric format reference is https://docs.fabricmc.net/develop/loader/fabric-mod-json.

The scan enumerates all eligible entries, returning at most 64 JAR observations
per result page. A temporary disk index preserves duplicate-ID detection across
pages without retaining the inventory in RAM. Cancellation is checked between
entries. Each file is checked as a single-link regular object
before and after reading; observable file changes suppress its metadata.
Directory identity is checked separately, with descriptor-anchored enumeration
on Linux and macOS. This is not a point-in-time filesystem snapshot.

Log comparison uses only a complete historical selection, and skips nested
rows and loader built-ins. Exact ID and version text is distinguished from a
different observed version, repeated disk IDs, and insufficient evidence.
Only a complete selection with no omitted rows can report that an ID was not
listed. A text match does not establish the selected archive's path, hash,
provenance, Modrinth identity, initialization or current activation.

The separate management label compares the exact path spelling with the same
game's enabled install records. It neither changes them nor verifies a hash.
Repeated records are reported distinctly. Incomplete record reads do not
produce a definitive absent result, and an absent record is not proof of manual
installation.

## Bethesda Plugins

For Skyrim SE and Fallout 4, diagnosis additionally returns `bethesdaPlugins`
when a complete or bounded `Plugins.txt` was read. It considers only enabled,
valid `.esp`, `.esm` and `.esl` names already declared by that file, then reads
the exact loose `Data/<name>` file through the safe regular-file handle. It
parses only the first `TES4` record and its bounded `MAST` subrecords. A
complete header with a physically absent master produces an observed
dependency finding tied to the `Plugins.txt` line; the file is never copied,
rewritten, reordered or treated as an installation plan.

The list is indexed across pages so a master on a later page still has the
correct enabled order. Header work is paged (4 MiB total per header batch), with
parser protections of 256 KiB per plugin header, 256 master names and 4096
subrecords. These per-object limits are not resume points for unsafe parsing.
Compressed or oversized TES4 data, malformed subrecords, truncated input,
unsafe master names, concurrent file replacement and unreadable files become
`limited`, `invalid` or `unreadable` evidence and do not become a missing
dependency claim. Missing or disabled entries in `Plugins.txt` remain separate
load-order observations. The evidence also records the one-based enabled order
and reports when a declared master is disabled or appears after its plugin.
Master names that are physically present but not in the list are reported as an
uncertainty, not as proof of a broken load order.

This is not a Bethesda load-order verifier. It does not inspect archive
contents, BSA/BA2 files, MO2/VFS mappings, plugin headers outside the declared
loose files, ESL light slots, FormIDs, LOOT rules, effective ordering or live
game state. The order check only compares `Plugins.txt` lines; it does not sort
or validate the loader's final order. A missing loose master can still be supplied by a non-default
profile or virtual filesystem, so the result remains an observation and the
repair suggestion still requires confirmation.

The TES4 list format reference is
https://github.com/Ortham/libloadorder/blob/master/src/load_order/asterisk_based.rs.

## Baldur's Gate 3 Profiles

Diagnosis checks a complete registered profile's `modsettings.lsx` with a
bounded XML parser (DTD and external resolution disabled). It recognizes the
`ModuleSettings` structure with `Mods` alone or alongside `ModOrder`. It reports
line-numbered missing, invalid or duplicate UUIDs and order entries without a
matching valid `Mods` declaration when that list is unambiguous to parse.
Malformed, unsupported or truncated input produces an explicit unknown rather
than a partial structural verdict.

These are observations about one profile file. The manager does not infer order
from a missing `ModOrder`, resolve a UUID to a PAK file, inspect mod
dependencies, or decide whether the game used that profile or loaded a mod.

Bethesda format reference: https://github.com/Ortham/libloadorder/blob/master/src/load_order/asterisk_based.rs

BG3 format references: the BG3 Mod Manager templates at
`1.0.10.0` (both lists) and `1.0.12.9` (`Mods` only), and Norbyte's LSX writer:

- https://github.com/LaughingLeader/BG3ModManager/blob/1.0.10.0/src/Core/DivinityApp.cs#L32-L34
- https://github.com/LaughingLeader/BG3ModManager/blob/1.0.12.9/src/Core/DivinityApp.cs#L32-L34
- https://github.com/Norbyte/lslib/blob/master/LSLib/LS/Resources/LSX/LSXWriter.cs#L100-L159
