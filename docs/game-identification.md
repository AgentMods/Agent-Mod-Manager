# Unknown Game Identification

The local marker scanner remains authoritative for adapter matching. Online
search supplies untrusted evidence only, never executable instructions or an
installation layout. This workflow does not need a configured language model
or search API key. The app-provided built-in search opens Bing and is independent
of the user-configurable Playwright MCP connection in Settings → Tools; that
MCP connection does not need to be configured or enabled for game identification.
Windows prefers the installed Microsoft Edge browser.

## Desktop Workflow

1. Check a chosen game directory in the library. For an unmatched candidate,
   open **更多操作 → 联网识别** on its scan card.
2. Review the local name signals and editable public query. A separate consent
   checkbox must be selected before the exact query is sent to Bing through the
   app-provided browser.
3. Choose a web result, correct the display name, and independently confirm
   directory registration. No candidate is automatically selected.

Changing the search query discards results and both consents. Changing the
selected candidate or display name clears registration consent. Closing the
dialog discards consent; a refreshed local fingerprint also resets its state.
Unavailable browser or search results never become fabricated matches.
Registration does not attach an adapter or trigger a download or installation.

## Backend Contract

- `preview_game_identity(directory)` returns local-only signals, local marker
  matches, a metadata fingerprint and a suggested query. No game content is read.
- `search_game_identity(directory, expectedFingerprint, publicQuery,
  confirmation)` requires the exact `SEARCH_GAME_IDENTITY` confirmation. It
  validates consent, cancellation, query and fingerprint before starting the
  built-in browser search. `needs_configuration` means the browser could not
  start or required components are missing; `unavailable` means a network,
  CAPTCHA or result-page parsing failure. Neither state produces candidates.
- `confirm_game_identity(searchId, candidateIndex, name, confirmation)` requires
  `CONFIRM_GAME_IDENTITY`. It reloads stored evidence rather than trusting
  caller-supplied results and rechecks the directory before and after the write.

The canonical root and ordinary immediate entries contribute to the fingerprint,
including metadata stamps. Scans reject more than 512 direct entries and expose
at most 32 name signals. They skip links and special files, do not recurse or
run programs, and reject oversized/unsafe names. Filesystem roots, manager-data
overlap and sensitive credential/system containers are denied. Standard
Steam/Minecraft exceptions are limited to specific game directories, not all
of macOS Library or Windows AppData.

Only the reviewed public query is entered into Bing through the app-provided
browser. Local paths, signals and game bytes are not added to the query.
Query validation rejects private paths, unsafe URL terms and known raw or
URL-encoded credentials.
Results reuse the bounded URL/text sanitization of the existing search service;
the manager never fetches the result pages automatically.

Search evidence lives in `game_identity_searches`. An available result can be
reused for 24 hours only for the same root, fingerprint and query. Unavailable
browser searches never create or expose fabricated candidates.
Record columns and a content digest are checked on load. The digest detects
accidental/stale record modification; it is not a signature against an attacker
who can rewrite the database and recompute it. New evidence invalidates earlier
evidence for that directory without deleting files or records.

Confirmation holds the global file-write lock and performs game registration,
metadata update and evidence consumption in one SQLite transaction. Existing
installation counts and unrelated metadata survive. Existing adapter identities
cannot be replaced. New online-only records have zero inferred confidence and
no guessed Steam App ID, platform or adapter. Evidence and explicit confirmation
are stored in `games.metadata.onlineIdentity`. Interrupted file recovery blocks
registration. A changed directory, expired evidence or invalid index fails closed.

## Agent Integration

`preview_game_identity` is local/read-only. `search_game_identity` uses the
publication effect so default and AutoApprove sessions require review of the
exact outbound query and Bing destination. FullTrust still obeys validation and
path restrictions. Users confirm an online identity result in the desktop library.
The separate `register_game` tool registers an explicitly requested local directory
using local evidence. It does not consume or confirm an online search result. Neither identity tool is automatically added
to the external MCP server's deliberate native-tool allowlist.

## Verification And Limits

Light/dark prototypes were generated and individually inspected before UI
implementation, using `model_providers.custom` and the fixed `gpt-image-2.5`
model. Exact prompts are in the ignored local artifact
`output/imagegen/agent-mod-manager-prototypes/identity-prompts.jsonl`; the two
saved images are in its sibling `identity/` directory. Implemented screenshots
are in `output/playwright/screenshots/*-identity-{light,dark}.png`.

Rust tests cover query consent, directory fingerprints, cache reuse and the
original source of saved evidence. The Agent integration test rejects a search
under both review policies. UI tests exercise consent, edited queries and
names, reopened dialogs, missing browsers, service failures and both themes
at 1280, 1440 and 390 pixels.

A Windows live test returned Bing results through the bundled runtime and
shared Rust search function. The full game-registration dialog was tested with
mocked IPC. Directory fingerprints record filesystem metadata. Descriptor-level
filesystem hardening remains open.
