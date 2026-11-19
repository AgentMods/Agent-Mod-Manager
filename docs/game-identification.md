# Unknown Game Identification

The local marker scanner remains authoritative for adapter matching. Online
search supplies untrusted evidence only, never executable instructions or an
installation layout. This workflow does not need a configured language model;
Brave Search requires the `sources.brave` credential in Settings.

## Desktop Workflow

1. Check a chosen game directory in the library. Low-confidence candidates
   offer online identification.
2. Review the local name signals and editable public query. A separate consent
   checkbox must be selected before the exact query is sent to Brave.
3. Choose a web result, correct the display name, and independently confirm
   directory registration. No candidate is automatically selected.

Changing the search query discards results and both consents. Changing the
selected candidate or display name clears registration consent. Closing the
dialog discards consent; a refreshed local fingerprint also resets its state.
Missing credentials and unavailable searches never become fabricated matches.
Registration does not attach an adapter or trigger a download or installation.

## Backend Contract

- `preview_game_identity(directory)` returns local-only signals, local marker
  matches, a metadata fingerprint and a suggested query. No game content is read.
- `search_game_identity(directory, expectedFingerprint, publicQuery,
  confirmation)` requires the exact `SEARCH_GAME_IDENTITY` confirmation. It
  validates consent, cancellation, query and fingerprint before accessing
  credentials or the network.
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

Only the reviewed public query reaches the fixed Brave endpoint. Local paths,
signals and game bytes are not added to the request. Query validation rejects
private paths, unsafe URL terms and known raw or URL-encoded credentials.
Results reuse the bounded URL/text sanitization of the existing search service;
the manager never fetches the result pages automatically.

Search evidence lives in `game_identity_searches`. An available result can be
reused for 24 hours only for the same root, fingerprint and query. Credentials
must still be present; a missing key does not make cached candidates available.
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
exact outbound query and destination. FullTrust still obeys validation and
path restrictions. The Agent cannot confirm or register the result; the user
must do so in the desktop library. Neither identity tool is automatically added
to the external MCP server's deliberate native-tool allowlist.

## Verification And Limits

Light/dark prototypes were generated and individually inspected before UI
implementation, using `model_providers.custom` and the fixed `gpt-image-2.5`
model. Exact prompts are in the ignored local artifact
`output/imagegen/agent-mod-manager-prototypes/identity-prompts.jsonl`; the two
saved images are in its sibling `identity/` directory. Implemented screenshots
are in `output/playwright/screenshots/*-identity-{light,dark}.png`.

The backend has 16 focused tests, the Agent tools have 4, and a mock OpenAI SSE
integration test rejects search approval under both review policies. Twelve
Playwright cases cover independent consent, changed queries/names, reopened
dialogs, missing keys, stale evidence and light/dark layouts at 1280, 1440 and
390 pixels. The full Rust and frontend suites also pass as recorded in the
implementation status document.

No live authenticated Brave search or native Windows/Linux acceptance has been
performed for this workflow. Fingerprints are metadata observations, not
descriptor-held filesystem authority or proof of game compatibility. The
project-wide descriptor-level TOCTOU hardening requirement remains open.
