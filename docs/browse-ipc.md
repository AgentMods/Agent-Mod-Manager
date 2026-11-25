# Desktop Mod browse IPC

These commands are desktop-only. `invoke` parameters and returned DTO fields use
camelCase. IDs are opaque and scoped to the invoking window and current storage.
Only the sources explicitly selected in `browse_authorize` can be queried;
discovery and consent do not launch or call MCP tools. A discovery can be used
for 3 minutes, an authorization for 30 minutes, and a search for at most 12
minutes (never beyond its authorization). An authorization never inherits a
replacement connection or credentials. A search accepts at most 8 games and a
240-byte UTF-8 query. Each page contains up to 24 items; at most 12 pages can be
requested.

For the stock 3DM HTTPS endpoint, guest access and a user-selected MCP credential
reference both support browsing. Changing the binding or its stored value
invalidates the old connection. For Nexus Mods, the backend can query the Steam
display title associated with the registered game's App ID and installation.
It falls back to the library name if the title lookup has no results. A unique
exact match supplies the Nexus domain for Mod queries. Ambiguous matches still
require selection. The local installation path stays on the desktop.

- `browse_list_sources()` → `Source[]`. Each source has `id`, `name`, and
  `status` (`available | notConnected | permissionDenied | unsupported`).
  Stable IDs: `glossmod_http`, `glossmod_3dm`, `modio`, `curseforge`,
  `gamebanana`, `nexusmods`, `thunderstore`.
- `browse_authorize({ sourceIds })` → `{ authorizationId, sources }`. Call only
  after the user confirms sources from a recent `browse_list_sources` call in
  that window. Changing a connection or expiry requires renewed confirmation.
  A new authorization replaces the prior one.
- `browse_start_search({ authorizationId, gameIds, query, sourceIds? })` →
  `{ searchId }`. `sourceIds` may select a subset; omitted means the authorized
  sources, not every newly connected source. `query: ""` asks for the browse feed.
  Don't call with no games. A new search replaces the window's current search.
  Each still-valid source can continue if another source disconnects.
- `browse_get_page({ searchId, page })` → `{ status, page, items, issues,
  gameChoices, hasNext, isEnriching }`. `page` is zero-based. Start at `0`, poll
  `loading` until `ready`, request `page + 1` only when `hasNext`, and restore
  previous page snapshots by their ordinal. Continue polling ready pages while
  `isEnriching` to receive localized public text. Enrichment keeps page
  membership and order. A subsequent read hides items from revoked sources.
  `hasNext` is continuation evidence, not a total; duplicate-only or exhausted
  continuations may finish with an empty page. The original query is searched
  first. If page 0 has original results, it becomes ready without waiting for
  translation. If it is empty, the backend automatically tries any translated
  alternatives on page 0 before returning an empty first page. Each alternate
  has its own continuation and never replaces the original query.
- `browse_select_game_match({ searchId, gameId, sourceId, providerGameId })`
  persists a mapping from the options returned for that pair. Restart with the
  same filters and authorization to apply it. The IPC does not accept arbitrary
  IDs or ask the model to infer game equivalence.
- `browse_open_project({ searchId, itemId })` opens the official canonical
  work page in the system browser. Calls take no URL. Use `canOpenProject` for
  presentation; it conveys neither download permission nor install success.
- `browse_cancel_search({ searchId })` discards the search and cancels its reads.
  Installation tasks launched while a search was valid have their own lifecycle.
- `browse_install_task({ searchId, itemId, gameId })` → `Task | null`.
  Looks up an existing attempt without starting Agent work.
- `start_browse_install({ searchId, itemId, gameId })` → `Task`. Requires a
  current browse search and a matched local game. Starting work does not grant
  approval for subsequent download or install actions.
- `list_browse_install_tasks()` returns `{ searchId, itemId, gameId, title }[]`
  for previously started work, independent of the current search.

`BrowseItem` has `{ id, sourceId, gameIds, title, originalTitle, author, summary,
typeKey, tags, coverUrl, canOpenProject }`. `id` is a per-search opaque item ID;
`gameIds` are registered game IDs matched through that provider. `author`,
`summary`, `coverUrl` may be null. `tags` are `{ key, label }`, and `typeKey` is
`visual | localization | gameplay | audio | framework | other | unknown`.
Install uses `{ searchId, itemId }` plus a `gameId` from that item's `gameIds`.
Apply type/tag filters to fetched data only.

`Task` contains `{ id, conversationId, turnId, status, installId, transactionId }`.
`turnId`, `installId` and `transactionId` may be null. Status is
`running | installed | unverified | rejected | cancelled | interrupted | failed`.
The task index supports reopening existing attempts (including awaiting
approval) after the search expires, but does not grant a new start.

`issue` has `{ gameId, sourceId, code }`: `noGameMatch`, `permissionDenied`,
`queryFailed`, `limitReached`. Unavailable sources are also reported at
source discovery; an empty ready page with no issues means no projects were
returned. `gameChoices` entries are `{ gameId, sourceId, options }`, where options
are `{ id, name }`; prompt for an explicit selection instead of guessing.
Session errors use native presentation keys, including `browse.authorization_changed`
and `browse.search_expired`; restart or reauthorize as appropriate.
