use crate::die;
use crate::Cli;
use clap::CommandFactory;
use clap_complete::Shell;
use std::path::PathBuf;

pub(crate) fn generate_completions(shell: Shell) -> String {
    let mut buf = Vec::new();
    clap_complete::generate(shell, &mut Cli::command(), "sema", &mut buf);
    let mut out = String::from_utf8(buf).expect("clap completion output is utf-8");
    if shell == Shell::Zsh {
        out = fix_zsh_root_completion(out);
    }
    out.push_str(dynamic_doc_completion_script(shell));
    out
}

/// Repair subcommand completion in the generated zsh script.
///
/// `clap_complete`'s zsh generator emits the top-level optional positionals
/// (`FILE`, `SCRIPT_ARGS`) *before* the subcommand slot — even with
/// `args_conflicts_with_subcommands` set — so zsh consumes `sema notebook` as
/// the FILE positional: `sema <TAB>` offers only files and
/// `sema notebook <TAB>` completes script arguments. Subcommand completion
/// never engages, at any depth.
///
/// The repair makes position 1 an alternation of subcommands and script files
/// (`_sema_root`), and re-indexes the subcommand dispatch from `$line[3]` to
/// `$line[1]`. Every rewrite is anchored on the exact generator output; if an
/// anchor is missing (a future clap_complete changed shape), the script is
/// returned UNMODIFIED — a wrong-but-consistent script beats a broken one —
/// and the pinning unit test fails loudly so the anchors get refreshed.
///
/// zsh is the ONLY affected shell: its generator dispatches by positional
/// index (`$line[N]`), while bash (word-walk), fish
/// (`__fish_seen_subcommand_from`), elvish and powershell (name-keyed maps)
/// all match literal subcommand names — verified empirically 2026-07-03
/// (bash 5.2 in a clean container; fish `complete -C`; pwsh
/// `CommandCompletion::CompleteInput`; elvish statically).
fn fix_zsh_root_completion(script: String) -> String {
    const POSITIONALS: &str = "'::file -- File to execute:_default' \\\n\
'::script_args -- Arguments passed to the script (after --):_default' \\\n\
\":: :_sema_commands\" \\\n";
    const ROOT_SLOT: &str = "\":: :_sema_root\" \\\n";
    let anchors_present = script.contains(POSITIONALS)
        && script.contains("words=($line[3] \"${words[@]}\")")
        && script.contains("case $line[3] in");
    if !anchors_present {
        return script;
    }
    let mut out = script.replacen(POSITIONALS, ROOT_SLOT, 1);
    out = out.replacen(
        "words=($line[3] \"${words[@]}\")",
        "words=($line[1] \"${words[@]}\")",
        1,
    );
    out = out.replacen(
        "curcontext=\"${curcontext%:*:*}:sema-command-$line[3]:\"",
        "curcontext=\"${curcontext%:*:*}:sema-command-$line[1]:\"",
        1,
    );
    out = out.replacen("case $line[3] in", "case $line[1] in", 1);
    // The definition must precede clap's self-invoking trailer
    // (`if [ "$funcstack[1]" = "_sema" ]; then _sema "$@" ...`): on the very
    // first TAB the file executes top-to-bottom and calls `_sema` right there —
    // a root fn appended after the trailer is not yet defined at that moment.
    let root_fn = "\n_sema_root() {\n    _alternative \\\n        'subcommands:sema command:_sema_commands' \\\n        'files:script file:_files'\n}\n\n";
    const TRAILER: &str = "if [ \"$funcstack[1]\" = \"_sema\" ]; then";
    if let Some(pos) = out.find(TRAILER) {
        out.insert_str(pos, root_fn);
    } else {
        out.push_str(root_fn);
    }
    out
}

fn dynamic_doc_completion_script(shell: Shell) -> &'static str {
    match shell {
        Shell::Bash => {
            r#"

# Dynamic Sema doc symbol completion.
_sema_doc_complete() {
    local cur="${COMP_WORDS[COMP_CWORD]}"
    if [[ ${COMP_WORDS[1]} == doc && ${COMP_CWORD} -eq 2 ]]; then
        COMPREPLY=( $(compgen -W "$(sema __complete-doc-symbols "$cur")" -- "$cur") )
        return
    fi
    if [[ ${COMP_WORDS[1]} == doc && ${COMP_WORDS[2]} == show && ${COMP_CWORD} -eq 3 ]]; then
        COMPREPLY=( $(compgen -W "$(sema __complete-doc-symbols "$cur")" -- "$cur") )
        return
    fi
    _sema "$@"
}
complete -o nosort -o bashdefault -o default -F _sema_doc_complete sema
"#
        }
        Shell::Zsh => {
            r#"

# Dynamic Sema doc symbol completion.
_sema_doc_complete() {
  if (( CURRENT == 3 )) && [[ "${words[2]}" == "doc" ]]; then
    local -a matches
    matches=("${(@f)$(sema __complete-doc-symbols "${words[CURRENT]}")}")
    _describe 'Sema doc symbol' matches
    return
  fi
  if (( CURRENT == 4 )) && [[ "${words[2]}" == "doc" && "${words[3]}" == "show" ]]; then
    local -a matches
    matches=("${(@f)$(sema __complete-doc-symbols "${words[CURRENT]}")}")
    _describe 'Sema doc symbol' matches
    return
  fi
  _sema "$@"
}
compdef _sema_doc_complete sema
"#
        }
        Shell::Fish => {
            r#"

# Dynamic Sema doc symbol completion.
complete -c sema -n '__fish_seen_subcommand_from doc; and not __fish_seen_subcommand_from show search apropos' -a '(sema __complete-doc-symbols (commandline -ct))'
complete -c sema -n '__fish_seen_subcommand_from doc show' -a '(sema __complete-doc-symbols (commandline -ct))'
"#
        }
        _ => "",
    }
}

pub(crate) fn install_completions(shell: Shell) {
    let home = match std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
        Ok(h) => PathBuf::from(h),
        Err(_) => {
            die("could not determine the home directory");
        }
    };

    let path = match shell {
        Shell::Zsh => home.join(".zsh/completions/_sema"),
        Shell::Bash => home.join(".local/share/bash-completion/completions/sema"),
        Shell::Fish => home.join(".config/fish/completions/sema.fish"),
        Shell::Elvish => home.join(".config/elvish/lib/sema.elv"),
        Shell::PowerShell => {
            die("Auto-install is not supported for PowerShell.\n\
                 Run manually: sema completions powershell >> $PROFILE");
        }
        _ => {
            die("auto-install is not supported for this shell");
        }
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap_or_else(|e| {
            die(format!(
                "could not create directory {}: {e}",
                parent.display()
            ));
        });
    }

    let completions = generate_completions(shell);
    std::fs::write(&path, completions).unwrap_or_else(|e| {
        die(format!("could not write {}: {e}", path.display()));
    });

    println!("✓ Installed {shell} completions to {}", path.display());
    if shell == Shell::Zsh {
        println!("  Add to ~/.zshrc (before compinit): fpath=(~/.zsh/completions $fpath)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zsh_completions_dispatch_subcommands_at_position_one() {
        let script = generate_completions(clap_complete::Shell::Zsh);
        assert!(
            script.contains(":: :_sema_root"),
            "root slot missing — anchor drift in fix_zsh_root_completion"
        );
        assert!(
            script.contains("_sema_root() {"),
            "root alternation fn missing"
        );
        assert!(
            script.contains("case $line[1] in") && !script.contains("case $line[3] in"),
            "top-level dispatch must read the subcommand from position 1"
        );
        assert!(
            !script.contains("File to execute"),
            "top-level FILE positional must not shadow the subcommand slot"
        );
        // The nested groups must still be intact (spot-check one). clap_complete
        // names nested subcommand groups `_sema__subcmd__<name>_commands`.
        assert!(script.contains("_sema__subcmd__notebook_commands"));
    }
}
