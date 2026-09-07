# Shared path resolution and filesystem publication

Status: Implemented for the defect-prone callers listed below. General file
operations keep their existing API.

## Problem

Sema had several independent definitions of path identity and containment.
Some canonicalized only the immediate parent of a missing path. For example,
`new/../app.sema` could compare differently from `app.sema` before `new` was
created, then name the same file after directory creation. Security checks also
handled symlinks in missing path prefixes differently.

Persistent files used several unrelated temporary-file patterns. Fixed names
such as `file.json.tmp` allowed concurrent writers to truncate each other's
temporary files. Some Windows paths wrote directly to the destination. The web
build also tried to publish an archive and a report as a pair even though two
filesystem renames cannot form one portable atomic transaction.

## Shared API

`sema_core::path` now owns:

- lexical normalization;
- absolute resolution against an explicit base;
- resolution of the deepest existing ancestor followed by a missing suffix;
- destination comparison, including filesystem identity for existing hard
  links; and
- containment resolution for paths that may not exist yet.

| API | Intent |
| --- | --- |
| `PathExt::resolve_allow_missing` | Resolve symlinks in the existing prefix while allowing a missing suffix. |
| `PathExt::is_same_destination_as` | Compare existing file identity or a future write destination. |
| `PathExt::absolute_from` | Build a lexical absolute path from an explicit base without resolving symlinks. |
| `PathBoundary` | Resolve a root once and validate candidate containment. |
| `PathBoundary::contains` | Distinguish an outside path from a filesystem resolution failure. |

Lexical normalization is an internal implementation detail rather than a
choice exposed to callers.

`sema_core::fs` is host-only and owns unique sibling temporary files, file
sync, atomic file replacement, parent-directory sync, preservation of existing
permissions, and owner-only creation for private files on Unix.

`AtomicFile` delegates staged replacement to `atomic-write-file` on non-Windows
hosts. The Windows backend retains explicit write-through replacement. The
wrapper keeps Sema's parent-directory creation, private-file policy, and
one-shot API.
`PathExt::resolve_allow_missing` delegates filesystem-aware canonicalization to
`soft-canonicalize`; Sema retains destination identity and boundary policy.
Native builds keep `soft-canonicalize`'s process-aware canonicalization so Linux
`/proc/<pid>/root` and `/proc/<pid>/cwd` paths do not lose their namespace
boundary. Browser WASM builds disable that host-only feature.

| API | Intent |
| --- | --- |
| `AtomicFile::new` | Stage an ordinary replacement for explicit later commit. |
| `AtomicFile::new_private` | Stage an owner-only replacement on Unix. |
| `AtomicFile::write` | Perform an ordinary one-shot atomic replacement. |
| `AtomicFile::write_through` | Atomically update a file while retaining a final symlink. |
| `AtomicFile::write_private` | Perform a one-shot replacement for a sensitive file. |
| `AtomicFile::commit` | Flush and publish an explicitly staged replacement. |

Errors from containment and collision checks are not converted to lexical
fallbacks. A caller that protects a security or overwrite boundary must fail if
it cannot establish the resolved path.

Ordinary content editors use `AtomicFile::write_through` to preserve the final
symlink behavior of direct file writes. Archives and sensitive stores use
`AtomicFile::write` or `AtomicFile::write_private` when replacing the named
directory entry is intentional.

## Migrated callers

- CLI source/output collision checks (native and web targets) and build
  outputs (`.semac`, executables, web archives);
- core sandbox allowed-path checks;
- workflow policy path normalization;
- `path/within?` path resolution;
- notebook VFS containment, including nested new files, and notebook saves;
- LSP workspace and document path identity;
- formatter source replacement (`sema fmt` and the MCP `fmt` tool);
- package manifests, lockfiles, metadata, and credentials;
- MCP OAuth token stores and MCP build/export outputs;
- LLM cassette snapshots and the LLM disk cache;
- `kv/flush`, `patch/apply-file`, workflow evidence bundles, and the dev
  server's `app.vfs`.

Import resolution, archive member validation, DAP URI parsing, embedded web
runtime validation, and the user-facing `file/write` family remain local. They
implement domain rules rather than general host path identity or file
publication, and `file/write` must keep working on FIFOs and device files,
which an atomic rename cannot target.

## Verification

Unit tests in `sema_core::path` and `sema_core::fs` cover missing-directory
`..` aliases, hard-link identity, atomic replacement, cleanup on an abandoned
write, private-file permissions, and symlink write-through. Sandbox and policy
containment tests, the CLI source-collision tests (including the web target,
which previously had no check and could overwrite its source), and the full
workspace suite pass on the migrated callers.
