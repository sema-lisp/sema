@AGENTS.md

## Sema workspace (if present)

If a `../CLAUDE.md` and `../repos.tsv` exist beside this repo, you are inside the
**sema-lisp workspace** meta-repo, and its `../CLAUDE.md` is MANDATORY here:

- Create/remove git worktrees ONLY via `jake wt-new` / `jake wt-rm` run from the
  workspace root — never `git worktree add` by hand, and never outside
  `../.worktrees/`.
- Rust builds use the workspace's sccache cache with incremental disabled
  (`../.cargo/config.toml`); reclaim disk with `jake sweep`. Don't re-enable
  incremental or drop the wrapper.

Read `../CLAUDE.md` before creating worktrees or running large builds.
