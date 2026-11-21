# Mod sources

All sources use the same `ModSource` search/resolve interface and the existing
download manager. Resolving is not permission to install. Missing or malformed
credentials disable only that provider. Search failures are isolated so another
provider can still return results.

## Catalog download binding

`resolve_mod` returns `download: null` for a manual or unavailable download.
Otherwise the preview includes `download.resolution_id`, source/project/version,
filename, size, source hashes, and warnings. It does not include the CDN URL.
The desktop IPC accepts `{taskId, resolutionId}` for `download_mod`. The Agent
and local MCP call it with `{"resolution_id":"…"}`. None accepts a caller-supplied
provider, version, URL, filename, size or checksum for a catalog download.

The backend keeps the exact resolved request in memory. IDs are single-use and
scoped separately to this desktop process, an Agent conversation, or an inbound
MCP session. They expire after at most ten minutes, or earlier on cache eviction
or restart. Retry or download another copy by resolving the exact release again.
The Agent and MCP approval preview comes from this same resolution; an expired,
unrecognized, already-used, or wrong-scope ID cannot start a transfer. A pinned
CurseForge request also rejects a returned project or file ID that does not match.

New catalog resolution replies, approvals, and structured tool history do not
contain CDN URLs. The Agent also omits legacy structured resolver links when it
rebuilds model context. Old on-disk history and arbitrary conversation text are
not rewritten. These changes do not retroactively verify the origin of old
downloads. `download_mod` does not accept uncataloged links.

## Reviewed public archive links

When a catalog cannot provide a download, the desktop Agent can use
`download_web_mod` with `{"url":"https://…/release.zip"}`. The link must be
explicitly provided or present in evidence; a search hit on a project page is
not a direct download. The tool requires per-transfer approval even in Full
Trust, shows the canonical URL, filename, and 1 GiB limit, and makes no download
request before approval. It is not a desktop IPC command or an exported local
MCP tool.

The starting link must be public HTTPS on the standard port, without userinfo,
query parameters, or a fragment. Supported suffixes are `.zip`, `.jar`, `.7z`,
`.tar.gz`, and `.tgz`. The filename comes from the approved URL, not response
headers. Public HTTPS redirects may go to another host. Each hop is checked and
DNS-pinned; server-provided CDN query parameters are used for transport, not
included in returned download data. Invalid tool arguments, including signed
starting links, are not preserved in structured tool history. A valid public
starting URL is visible in its approval and history; do not submit private links.

Downloads are capped, record local hashes, and check the expected archive
signature before promotion. A mismatched payload retains its `.part` bytes and
a rejection marker, not an installable artifact. The header check is not a full
archive preflight, publisher verification, malware scan, or compatibility check.
Valid artifacts appear in Downloads as unverified and review-required; the record
has source `web` and no catalog project or version. Installing goes through the
existing archive and file-plan workflow; a web artifact cannot claim a provider
origin. For logged-in, query-bearing, or unsupported links, download from the
provider website and use reviewed local import.

On 2026-11-19 the direct HTTPS transport downloaded the pinned Sodium JAR
into a temporary manager directory. Its 1,297,415 bytes matched the previously
recorded local SHA-256; the new row still correctly reported `web`, no provider
version, and no upstream verification. Nothing was extracted or installed.
The Agent approval and refusal paths are separately tested with scripted SSE
and an inert loopback ZIP.

```sh
cargo test --manifest-path src-tauri/Cargo.toml --locked --lib \
  opt_in_live_public_archive_has_no_catalog_provenance -- --ignored --nocapture
```

## Nexus Project IDs

Nexus numeric Mod IDs are scoped by game. Search accepts either a numeric query
or a bounded text query with `game_ids["nexus"]`; text search uses the fixed,
read-only GraphQL query at `api.nexusmods.com/v2/graphql`. The game domain and
search term are sent as JSON variables, never interpolated into the query.
Game-version and loader filters are reported as unsupported instead of being
silently ignored. Results and downloads record `domain/id` as the project ID,
and a result from another game, duplicate ID, malformed node, inconsistent
count, or GraphQL response containing errors is rejected without partial
results. Offset pagination is preserved and the source status indicates when
more results exist.

Resolve accepts that scoped ID or the legacy numeric form with `game_id`. A
scope inconsistent with `game_id` is rejected before any request. Older
numeric-only archive origins are not eligible for version replacement because
they do not identify the originating game's project. Nexus API Key setup and
account download permissions remain separate from read-only text search.

## Provider version declarations

`list_mod_versions` exposes a bounded, paged list of release metadata to the
desktop, Agent and local MCP for Modrinth and Thunderstore. Modrinth filters by
the original project ID and any requested game version/loader before pagination;
releases without files are omitted. Thunderstore validates the requested
community and project first, then reads its fixed-page version endpoint without
following provider-supplied URLs. Thunderstore does not declare game versions or
loaders in this response, so those filters are reported as `limited` rather than
silently inferred. Items contain the exact version ID, display label, publication
date and release channel; they do not include download URLs, hashes or release
notes. Listing uses the existing provider request cache and never reads API
keys. Other known sources return `unsupported`.

Use the exact version ID, original project and same filters to resolve a listed
version. A failed or mismatched resolution never falls back to another release.
Downloading the resolved archive and approving an install or replacement plan
are separate actions.
Source claims do not establish loader compatibility or identify an available
update. Primary schemas: https://docs.modrinth.com/api/operations/getprojectversions/
and the Thunderstore Cyberstorm package-version endpoint.

`resolve_mod` returns `dependencies` for the selected Modrinth version. It
preserves the provider's `required`, `optional`, `incompatible` and `embedded`
types and the nullable `project_id`, `version_id` and `file_name` fields. A
filename is a display reference, not an install path or a download location.
A malformed, missing or oversized list is unknown (`null`), not an empty list
(`[]`). Other providers do not currently report dependencies through this field.

A pinned dependency can be looked up with `source_id: "modrinth"` and its
exact `version_id`, even if no project ID was declared. The response must match
that version ID and any supplied project, loader and game version filters.
Do not fall back to another version if those checks fail.

Use `check_modrinth_dependencies` to compare the selected exact version's
declarations with the game's [install journal](install-lifecycle.md#modrinth-dependency-review).

### Transitive dependency metadata

`resolve_modrinth_dependency_graph` is available to Agent, local MCP and desktop
IPC. Inputs are exact root `version_id`, optional root `project_id`, explicit
`game_version` and `loader`, and optional `project_pins` mapping project IDs to
user-selected exact version IDs. No local game ID, path, or API key is sent.
The separate desktop IPC `review_modrinth_dependency_graph` binds the root to a
completed download record and adds install journal evidence and bounded local
file checks for exact enabled matches. Neither local path nor
local file evidence is sent to Modrinth. The generic graph tool does not inspect
installed files.

The graph follows required dependencies breadth-first. Pinned declarations
are verified against their exact version responses. For project-only edges,
it follows an unambiguous compatible release, otherwise it reports no match
or shows up to five candidates without selecting one. Use `list_mod_versions`
to browse more and pass the chosen version in `project_pins` to resume. Pins
never override a provider's exact declaration. Optional, embedded,
incompatible, and filename-only references are shown without automatic lookup.

The graph reports cycles, multiple selected versions of a project, and
incompatible declarations that match a selected node. Its limits are eight
levels below the root, 32 selected nodes, 256 edges, 48 logical lookups and
25 seconds. Shared lookups are deduplicated. Limits, invalid or missing
declarations, unresolved required edges and observed conflicts prevent a
`complete` result. `complete` means the required metadata was traversed,
not that downloads are available, local files match, or a loader will run it.
The tool never downloads, installs, or approves file changes.

Primary schema: https://docs.modrinth.com/api/operations/getversion/

On 2026-11-18, a read-only check of the public project-version list and exact
version endpoints returned matching project/version IDs and required/optional
dependencies for one Fabric 1.21.1 release. The application contract tests use
synthetic fixtures; no archive was fetched or installed in that check.
The same endpoints were also checked for an older beta chosen from a filtered
23-release public list: its exact ID, project, channel, game-version and loader
metadata matched the individual release response. This is endpoint evidence,
not a live application download or compatibility test.

On 2026-11-18, a standalone invocation of the production graph code over the
anonymous public API resolved the exact Sodium dependency of Iris `A99vfwJc`,
reported `choice_required` for Iris `zsoi0dso`, and resolved the latter after
explicitly pinning Sodium `26nVNc41` for Fabric 1.21.1. Only metadata was read;
no JARs were fetched, no local game was used, and no loader was launched.

### Live download and deployment

The opt-in test runs the real agent loop with a scripted loopback SSE model,
the public Modrinth API, and its CDN. One request searches Sodium, resolves
its pinned Fabric release (`AANobbMI` / `26nVNc41`), and waits for separate
download and installation approvals. A second turn requests rollback. All
files and the application database are in temporary directories; no API key,
existing game profile, or loader is used.

```sh
cargo test --manifest-path src-tauri/Cargo.toml --locked --lib \
  opt_in_live_modrinth_agent_install_and_rollback -- --ignored --nocapture
```

On 2026-11-18 this passed on macOS ARM64. The transferred JAR was 1,297,415
bytes and matched the pinned Modrinth SHA-512. Its local SHA-256 was
`411c4c03167066f0e2e61ba17793b4be13ddd73de5b677c6d402f1d8f4262ac2`.
The test confirmed the target stayed unchanged through download and preview,
the approved install matched the downloaded bytes and retained exact catalog
IDs, and the approved rollback restored the original fixture bytes without
removing the download. Neither the JAR nor a game was executed. This is
integrity and transaction evidence, not a security review of the JAR, a game
compatibility check, or acceptance of a live LLM.

## Thunderstore exact download

Search uses a Thunderstore community identifier, such as `valheim`. It does
not filter by a game version or loader; a text hit is not an exact package
match. Resolve checks the returned community, namespace, package, exact release
and first-party download path. An older version's `latest_version_number` is
not its selected version. A mismatch is rejected instead of downloading a
different ZIP. The project keeps its listing namespace; the release name and
download route use the owning team supplied by the provider.

On 2026-11-19, the real adapter searched for `HudToggle` in `valheim`, resolved
`HansenbansenTeam/HudToggle` at `1.0.1`, and downloaded 100,869 bytes through
the release endpoint and CDN. No source hash was supplied. The locally
recorded SHA-256 was
`df4297399b793ad689907a507fe9f7448c942c4c9b0687d5469fa6cd162d6da8`, not a
provider integrity or authenticity proof. The artifact required review, and
no game root, extraction, installation, or file transaction was created.

```sh
cargo test --manifest-path src-tauri/Cargo.toml --locked --lib \
  opt_in_live_thunderstore_download_stays_inert -- --ignored --nocapture
```

## 3DM API v3

Configure new 3DM connections under **MCP 服务** in Settings. The direct adapter
can still read an existing `sources.3dm` credential. It uses only
`https://mod.3dmgame.com/api/v3/mods` and `/mods/<id>`, with the raw key in the
sensitive `Authorization` header, not in a URL or `Bearer` prefix. Redirects
are disabled for API requests. The shared HTTP cache isolates credential scopes
and applies the existing rate limit and response-size limits.

The direct source does not start an MCP process, connect a server, reuse a
Mod 客栈 credential, or infer connection consent. The optional `uvx glossmod-mcp`
preset remains a separate user-confirmed, unsandboxed stdio connection.

Search uses the provider-native numeric `game_ids["3dm"]` when supplied, not a
Steam App ID or adapter slug. Search text is limited to 100 Unicode characters.
Offsets are translated into one or two native pages without silently rounding
down. Results bind the returned game ID to the requested game filter.

Resolve validates the project/game/resource associations. `version_id` means
the numeric **resource ID**, not the displayed version string. An explicit
resource ID never falls back to another file. Without an explicit resource,
exactly one first-party direct resource marked `mods_resource_latest_version`
must remain. Multiple eligible files require website selection rather than
guessing which optional patch to install.

Only observed HTTPS file-service paths on `dmod.3dmgame.com`, under
`/static/upload/resource/` or `/static/upload/resource2/`, are classified as direct
downloads. Cloud shares, other hosts, credentials, local URLs, signed/unknown
query forms and unsafe filenames do not become automatic download candidates.
The original Mod page remains available for manual selection. A direct resource
still goes through the existing DNS, redirect, size, cancellation and quarantine
checks. API credentials are never passed to the download host.

The verified response supplies rounded strings such as `"0.14MB"`, not an exact
byte count, and no file hash. The adapter deliberately leaves `size` and hashes
unset. A locally computed hash is not evidence of authenticity. Downloaded files
retain mandatory review and do not install themselves.

The response has no verifiable structured game-version/loader compatibility
contract. Requests with these filters return `unsupported` for search or
`manual_download` for resolution, without sending a request. A `latest` flag
does not prove compatibility, dependency satisfaction or file safety.

## Contract Evidence

Primary implementation:
https://raw.githubusercontent.com/GlossMod/gloss-mod-mcp/main/glossmod_mcp/server.py

On 2026-11-18, read-only `tools/list`, `get-mod-list` and `get-mod-detail` requests
to the public `https://mod.3dmgame.com/mcp` endpoint confirmed the nested v3
response wrapper and `mods_*`/`mods_resource_*` fields. An unauthenticated REST
request returned `success:false`; it was not counted as successful source
acceptance. No archive was downloaded, no account was logged in, and no
authenticated REST request was tested.

Offline fixtures are synthetic and retain the verified field names and envelope,
not copied Mod descriptions, live tokens or live download links. Contract tests
exercise the actual provider with an in-memory transport that asserts fixed
origins and sensitive headers and cannot fall through to the network. Live
key-authenticated REST and real archive acceptance remain outstanding.
