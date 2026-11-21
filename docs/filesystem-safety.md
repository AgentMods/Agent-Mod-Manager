# Filesystem operation safety

Root-restricted operations still check canonical paths against the manager data
root or registered game roots, as applicable. The desktop Agent's native
`read_dir`, `read_file`, `extract_preview`, `plan_file_ops`, and
`apply_file_ops` can work outside those roots with the desktop process's OS
permissions. Inbound MCP retains its existing scope. Credential directories
and `.env` files remain protected, and AutoApprove still requires review for
external writes.

Only the desktop Agent's native `plan_file_ops` accepts `write_text`. Its `dst`
and `content` fields specify a new file or the complete replacement contents,
not an append or patch. The plan fingerprints the UTF-8 bytes and permits at
most 1 MiB of combined text content per plan. It does not write the destination
until `apply_file_ops` runs under the approval policy. AutoApprove still
requires review for a plan containing `write_text`, including on registered
paths. Inbound MCP and other generic planners reject it. Apply stages and
verifies the payload and uses the same journal, overwrite backups, and rollback
checks as other file writes.

The transaction engine still checks the approved plan's fingerprints and
persists intent before doing filesystem work. These checks are not interchangeable
with safe path resolution: an outside process can change a name between the
check and the operation. `src-tauri/src/safe_fs.rs` is used by transaction
copy/move/mkdir, backup and rollback, manager metadata creation, transaction
hashing, and text opens for file tools and diagnosis.

- macOS and Linux open each canonical directory component using its parent's
  descriptor, with `DIRECTORY` and `NOFOLLOW`. New payloads use exclusive
  creation. Regular-file reads check the opened descriptor's type and identity;
  a path changing to a pipe cannot block the open. Moves use an exclusive
  kernel rename on the held parent descriptors, not a check-then-replace rename.
  Filesystems that cannot provide the exclusive rename fail closed. A
  cross-volume error still goes to the existing copy, verify, and retain path.
- On Windows, the file engine starts with a checked local drive root and opens
  each single name relative to its parent handle (`NtCreateFile`). It refuses
  reparse points, ambiguous names, alternate data streams and short-name aliases.
  Ancestor handles omit delete sharing while the operation resolves paths.
  Renames operate on the same opened source that passed the file-ID check and
  a held target directory (`NtSetInformationFile`). They never overwrite or
  request copy-and-delete. The Fabric disk scan also enumerates by checked
  directory handle rather than reopening its pathname.
- Rollback checks the persisted destination, source, backup and retained names
  again. If they no longer resolve to their original canonical spelling, it
  refuses to follow them, including into another already-authorized directory.
  A failed rollback remains recoverable after the original path is restored;
  it does not authorize another operation meanwhile.

## Transaction Recovery

Copies, explicit directories, and implicit parent directories are prepared in
transaction storage before promotion. Each exclusive rename of a game payload
or original has its own durable intent, including the source object's volume
and file ID (and a birth timestamp where provided on Unix). Recovery distinguishes
an unexecuted rename, an applied rename, and an already reversed rename by
checking where that object is, not by assuming the public destination belongs to
the transaction. Matching contents alone are not enough. Partial staging stays
in transaction storage; rollback does not delete it.

For cross-volume moves, the destination copy is verified and promoted before the
original is moved to same-volume retention. An overwrite's cross-volume backup
copy is kept for inspection; automatic recovery restores the identified retained
original, not a file that merely has the same checksum. A competing file stays
where it appeared. If it prevents restoration, the transaction stays pending and
blocks new plans until the conflict is resolved. A populated implicit directory
stays in place. A requested rollback of a committed transaction checks ownership
before making any change, so a stale or out-of-order request does not itself
block later writes. The desktop transaction-history command and the local Agent
can request rollback of an external file transaction. Rollback uses the scope
recorded with the transaction; inbound MCP cannot roll it back even if it
knows the transaction ID.

The forced-process-exit regression runs an actual worker against a temporary
WAL database. The worker has already created an additional payload and its
implicit parents, moved the original of an overwrite to backup, and persisted
the intent to promote the replacement. The parent kills the worker before
that last rename and reopens the database. Recovery must restore the original
file identity and bytes, retain the added payload outside the game tree,
remove the now-empty implicit parents, and mark the transaction rolled back.
A second recovery must do nothing. This is process death, not power loss.

Older journal rows remain readable for history and retention. Rows without
identity records cannot safely be reversed automatically. A committed legacy
transaction remains committed when such a rollback is refused; an interrupted
legacy transaction with unproven mutations requires manual inspection and stays
blocked. A replacement backup reconstructed from identical bytes is not the
original object and will not pass automatic rollback validation.

## Archive staging

`extract_preview` opens a regular archive for read-only listing; it does not
extract payloads or install them. Desktop Agent archive listing can use paths
outside managed roots. `preview_fomod` still requires an archive in managed
downloads or staging and does not write to the game.

Verified installation uses the same open archive for a read-only preflight and
the extraction pass. It checks SHA-256 through that handle before and after
parsing and compares the entry lists. It cannot open a different archive because a filename changed between
passes. Entries still undergo the path, type, byte-limit, and ratio checks.

On macOS and Linux, extraction creates a private, exclusive staging directory
under an existing no-follow parent and holds the directory descriptor until
extraction finishes. Entry paths are relative to that descriptor, with
no-follow directory traversal and exclusive file creation. An identity check
rejects a replaced staging root before the output paths can be returned. On
Windows, staging likewise holds the new directory and its ancestors, traverses
archive names relative to that handle, and verifies the root's file ID before
returning the staged paths.

Extraction hashes the exact bytes it writes. The install preview compares
each planned source's canonical path and SHA-256 to those results, or to the
original hash for a single-file install. A changed or relocated payload cannot
become a reviewed plan. A validation failure leaves partial staging in place
for inspection or the existing trash flow; it does not modify game files.

## Snapshot planning

Snapshot creation and restore use the exact registered root, not a replacement
link's target. Directory listing uses an identity-checked handle; regular files
are hashed through a verified handle. Before persisting a plan, the snapshot
code compares the planner's source and target observations with the inventory
and rechecks the scanned directories and the registration. Transaction retention
is excluded, and empty directories are represented in the manifest. The apply
step still uses the ordinary transaction checks.

This is not a point-in-time filesystem snapshot. External edits may race the
scan. A real directory already substituted at the same registered pathname
before scanning is not detectable from the registration alone.

## Remaining limits

These measures are not a sandbox against arbitrary same-user filesystem edits.
They do not pin the identity of a registered root across restarts or prevent
in-place edits to opened regular files. On Unix a held directory can still be
moved, and the source-leaf check and rename are distinct operations. New rename
records refuse multiply linked regular files. Object checks rely on stable local
filesystem IDs; without a birth timestamp, IDs may be reused across restarts.
Windows support here requires a local drive-letter root with stable file IDs.
UNC paths, mapped shares, substituted folders, and reparse-point subtrees
(including volume mount points) are not followed. Other directory walks, such as
game discovery, still need separate hardening.
Transaction storage must share the volume of paths promoted or restored by
exclusive rename; unusual nested mount layouts can fail closed. Tests include
a forced process exit and simulated cross-volume errors, not power loss or
real multi-volume Windows filesystems.

Regression cases use temporary directories. An Alpine arm64 container exercised
the same primitive module, including identity-checked renames, a collision,
hard links, nested links and a replaced staging root (not the desktop app). The
Windows MSVC cross-check compiles the implementation and Windows regression
cases for real junctions, staging-parent pinning, identity, enumeration and
no-replace rename. These cases are wired into the Windows CI test job, but the
cross-check does not execute them or verify the Windows installer.

Windows API references: [relative create](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/wdm/nf-wdm-zwcreatefile),
[relative rename](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_rename_information),
and [handle enumeration](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getfileinformationbyhandleex).
