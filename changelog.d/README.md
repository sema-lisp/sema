# changelog.d — one file per user-visible change

`CHANGELOG.md` is assembled from this directory at release time. **Never write a
new entry into `CHANGELOG.md` directly.** Adding an entry means creating a new
file here, which cannot fail on an ambiguous string match and cannot conflict
with another branch's entry.

Fixing an entry depends on whether it shipped. If it is still a fragment in this
directory, edit the fragment. If it is already in a released `## X.Y.Z` section,
edit `CHANGELOG.md` — assembly is one-way and the fragment no longer exists.
`changelog-release` never rewrites an existing version's section, so a manual fix
there is not at risk of being clobbered.

## Adding an entry

Create `<type>-<slug>.md`, where `<type>` is one of:

    added  changed  deprecated  removed  fixed  security

The slug is lowercase, digits and hyphens only. Examples:

    added-proc-run.md
    fixed-notebook-title-sizing.md

The file holds the entry body as a markdown bullet, in the same curated prose
style as the existing `CHANGELOG.md` sections: state what changed, why it
changed, and what it replaces. A commit subject is not enough — the changelog
explains the change to a user who did not read the diff.

    - **`proc/run` runs a child on the parent's terminal (#95).** There was no
      way to hand the terminal to an interactive program: `shell` captures
      stdout and stderr into pipes. `(proc/run ["nvim" path])` blocks until the
      child exits and returns its exit code.

A leading `- ` is optional; the assembler adds one if it is missing.

## Commands

| Command | Effect |
| --- | --- |
| `jake release.changelog-preview` | Render the next section to stdout. Writes nothing. |
| `jake release.changelog-release version=X.Y.Z` | Prepend the section to `CHANGELOG.md` and delete the fragments. |

`release.changelog-release` is step 3 of the Release Procedure in `AGENTS.md`. It refuses
to run if `CHANGELOG.md` already has a section for that version, if the version
is not `X.Y.Z`, or if a fragment filename does not parse.

This `README.md` is ignored by the assembler and is never deleted.

The assembler is `scripts/changelog.sema`, written in Sema and run by the
installed `sema` binary. It can also be called directly:

    sema scripts/changelog.sema -- preview
    sema scripts/changelog.sema -- release X.Y.Z
