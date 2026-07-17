# Shell-script hygiene for scripts/*.sh. Namespaced as `sh`.
#
# `sh.check` is the sanity gate: shellcheck (correctness + portability warnings)
# plus an shfmt formatting check. `sh.fmt` rewrites scripts to the canonical
# style (2-space indent, case-indent). Run `sh.check` before committing shell.

@group sh
@desc "Lint scripts/*.sh with shellcheck (correctness + portability)"
@needs shellcheck "brew install shellcheck"
task lint:
    # -S warning: gate on real bugs + portability, not subjective info/style.
    shellcheck -S warning scripts/*.sh
    echo "shellcheck: clean"

@group sh
@desc "Check scripts/*.sh formatting without writing (shfmt -d)"
@needs shfmt "brew install shfmt"
task fmt-check:
    shfmt -i 2 -ci -d scripts/*.sh
    echo "shfmt: clean"

@group sh
@desc "Reformat scripts/*.sh in place (shfmt -w, 2-space, case-indent)"
@needs shfmt "brew install shfmt"
task fmt:
    shfmt -i 2 -ci -w scripts/*.sh
    echo "shfmt: formatted scripts/*.sh"

@group sh
@desc "Shell sanity gate: shellcheck + shfmt formatting check"
task check: [lint, fmt-check]
    echo "sh: scripts clean"
