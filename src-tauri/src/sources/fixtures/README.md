# Provider response contracts

These are synthetic, anonymized examples following the official response schemas,
not live API captures. No fixture contains a real API key or live download token.
Extra documented fields are retained so tests exercise the upstream envelopes,
numeric IDs, nullable fields and casing rather than only our DTO layout.

- Modrinth: `GET /v2/search` (`hits`), project version list (array), and single
  version (one version object). Primary file hashes use `hashes.sha512` and
  `hashes.sha1`; every supplied hash is checked.
  References: https://docs.modrinth.com/api/operations/searchprojects/ and
  https://docs.modrinth.com/api/operations/getprojectversions/
- Thunderstore: Cyberstorm community listing (`results`, fixed pages of 20) and
  version detail. Detail uses `full_version_name`; `latest_version_number` can
  differ when an older version is requested. References:
  https://github.com/thunderstore-io/Thunderstore/blob/master/django/thunderstore/api/cyberstorm/views/package_listing_list.py
  and the adjacent `package_listing.py` and `serializers/package.py`.
- CurseForge: search (`data` array), mod (`data` object), file list (`data`
  array), and one file (`data` object). `allowModDistribution` and nullable
  `downloadUrl` control whether third-party download is possible. File hashes
  use `hashes[].algo` (1 = SHA-1, 2 = MD5) and `hashes[].value`. Both algorithms
  are checked only for legacy weak integrity, never authenticity or file safety.
  Reference: https://docs.curseforge.com/rest-api/
- Nexus: mod object, files (`files`, `file_updates`), and download-link array
  with uppercase `URI`. Ambiguous/archived files and forbidden or empty download
  links require website selection/download. These responses provide no file MD5;
  do not confuse the separate MD5 lookup endpoint with a file hash field. Reference:
  https://github.com/Nexus-Mods/node-nexus-api/blob/master/src/Nexus.ts
- 3DM API v3: search (`success`, `data.data`, `data.page`, `data.pageSize`)
  and detail (`success`, `data` object). Actual fields use `mods_*`, `game_id`
  and `mods_resource_*`; resource IDs are distinct from project IDs. Rounded
  `mods_resource_size` strings are not exact bytes and no file hash was observed.
  The synthetic examples include a first-party resource and a cloud-share mirror.
  Primary implementation:
  https://raw.githubusercontent.com/GlossMod/gloss-mod-mcp/main/glossmod_mcp/server.py
  Field names/envelopes were cross-checked with public read-only MCP calls on
  2026-11-18. No key-authenticated REST or archive download was performed. See
  [source restrictions](../../../../docs/mod-sources.md).

`provider_contract_tests.rs` installs an in-memory response queue only under
`cfg(test)`. Tests invoke the real provider `search`/`resolve` methods and inspect
their constructed requests. The queue never sends network requests, cannot fall
through when exhausted, and asserts the original fixed HTTPS origin and sensitive
authentication headers. Production code has no configurable endpoint or mock.
