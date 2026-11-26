# Desktop Mod browse IPC

These commands are desktop-only. `invoke` parameters and returned DTO fields use
camelCase. IDs are opaque and scoped to the invoking window and current storage.
On first entry, Browse automatically creates a read-only catalog grant for all
connected sources reported as `available` and loads the feed for the selected
games. Discovery and grant creation make no MCP tool calls and start no
connections. If no sources are available, Browse offers Settings and a source
reload. A source-loading error offers Retry.

The right-hand Choose sources dropdown holds a multi-select draft. Apply commits
the selected sources and starts a search in that scope. If any selected source
is no longer available, Apply leaves the existing search scope and results in
place and the user can revise the selection or retry. Normal connection checks
still apply to results from the existing scope. Cancel or closing the dropdown
discards the draft. The left-hand source filter selects All sources or one
source from the applied scope for the current results.

A discovery can be used for 3 minutes, an authorization for 30 minutes, and a
search for at most 12 minutes (never beyond its authorization). Each grant is
bound to the connections and credentials in its discovery. A search accepts at
most 8 games and a 240-byte UTF-8 query. Each page contains up to 24 items, and
at most 12 pages can be requested.

Each authorization keeps up to 64 successful game-directory pages and 64 Mod
result pages in memory for five minutes from retrieval. Repeated requests
within the same authorization reuse pages by source, game, query and cursor.
Failed reads can be retried immediately. Every new search checks the current
connection and issues fresh item IDs for opening and installation.

For the stock 3DM HTTPS endpoint, guest access and a user-selected MCP credential
reference both support browsing. Changing the binding or its stored value
invalidates the old connection.

Game lookup tries the registered library name and, when available, the title
from the verified Steam installation. A remembered manual choice or a unique
exact catalog match takes priority. If those fail, the configured model can
suggest localized titles, abbreviations and a base title to query through the
same authorized MCP connection. Each game shares its alias lookup across
sources. The model may identify a unique edition-equivalent title by position
in a complete set of observed candidates. The backend resolves that position
to a current provider ID, checks edition evidence and uses it for this search.
Model suggestions never write a user-confirmed mapping. Incomplete results,
conflicting editions and unresolved candidates remain available for manual
selection. Without a model the desktop still uses both known titles. Only
public names enter the model request; installation paths stay on the desktop.

- `browse_list_sources()` → `Source[]`. Each source has `id`, `name`, and
  `status` (`available | notConnected | permissionDenied | unsupported`).
  Stable IDs: `glossmod_http`, `glossmod_3dm`, `modio`, `curseforge`,
  `gamebanana`, `nexusmods`, `thunderstore`.
- `browse_authorize({ sourceIds })` → `{ authorizationId, sources }`. Supply
  available IDs from a recent `browse_list_sources` call in that window. On
  first entry Browse uses every available ID. Applying a new source scope uses
  the applied IDs. If the grant expires or a connection changes, refresh
  discovery before requesting a replacement. A successful authorization replaces
  the prior grant and cancels its current search. An unavailable source rejects
  the replacement and leaves the existing grant and search in place.
- `browse_start_search({ authorizationId, gameIds, query, sourceIds? })` →
  `{ searchId }`. `sourceIds` may select a subset; omitted means the authorized
  sources, not every newly connected source. `query: ""` asks for the browse feed.
  Don't call with no games. A new search replaces the window's current search.
  Each still-valid source can continue if another source disconnects.
- `browse_get_page({ searchId, page })` → `{ status, page, items, issues,
  gameChoices, hasNext, isEnriching }`. `page` is zero-based. Start at `0`, poll
  `loading` until `ready`, request `page + 1` only when `hasNext`, and restore
  previous page snapshots by their ordinal. Continue polling ready pages while
  `isEnriching` to receive primary categories and localized public text. Enrichment keeps page
  membership and order. A subsequent read hides items from revoked sources.
  `hasNext` is continuation evidence, not a total; duplicate-only or exhausted
  continuations may finish with an empty page. The original query is searched
  first. If page 0 has original results, it becomes ready without waiting for
  translation. If it is empty, the backend automatically tries any translated
  alternatives on page 0 before returning an empty first page. Each alternate
  has its own continuation and never replaces the original query.
- `browse_select_game_match({ searchId, gameId, sourceId, providerGameId })`
  persists a mapping from the options returned for that pair. Restart with the
  same filters and authorization to apply it. The selected ID must be present
  in the choices returned by the backend.
- `browse_open_project({ searchId, itemId })` opens the official canonical
  work page in the system browser. Calls take no URL. Use `canOpenProject` for
  presentation; it conveys neither download permission nor install success.
- `browse_cancel_search({ searchId })` discards the search and cancels its reads.
  Installation tasks launched while a search was valid have their own lifecycle.
- `browse_install_task({ searchId, itemId, gameId })` → `Task | null`.
  Looks up an existing attempt without starting Agent work.
- `start_browse_install({ searchId, itemId, gameId })` → `Task`. Requires a
  current browse search and a matched local game. The card invokes this command
  on click for a single match or when the user picks a game from multiple
  matches. It returns an existing running or installed task for the same work
  and game. Installation runs in the background. The user can open the task list
  to follow progress or answer a version question. The first turn inherits the
  saved permission mode. Follow-up and retry turns in Agent chat recover the
  original game and source project from the native task record and use the usual
  chat permission settings. When the source has already delivered a file,
  the Agent can recover its verified download receipt and continue archiving.
- `list_browse_install_tasks()` returns `{ searchId, itemId, gameId, title }[]`
  for previously started work, independent of the current search.

`BrowseItem` has `{ id, sourceId, gameIds, title, originalTitle, author, summary,
typeKey, tags, coverUrl, canOpenProject }`. `id` is a per-search opaque item ID;
`gameIds` are registered game IDs matched through that provider. `author`,
`summary`, `coverUrl` may be null. `tags` are `{ key, label }`, and `typeKey` is
one of `visual`, `localization`, `gameplay`, `audio`, `framework`, `tools`,
`fixes`, `interface`, `vehicles`, `characters`, `performance` or `other`.
The desktop first derives a category from public source metadata. The configured
Agent then classifies every visible item that lacks a current classification in
its cache, including items with a source-provided category. Results become
available as soon as classification finishes, while translation proceeds.
Category keys stay the same across languages. The card badge and type filter
use the same key, including `other` for entries with too little information.
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
