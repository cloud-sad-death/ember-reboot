//! zsh integration. Ember points ZDOTDIR at its own folder only long enough to load the user's real
//! .zshenv; ZDOTDIR is restored at once, so the user's .zprofile/.zshrc/.zlogin, prompt, completions,
//! aliases and functions load exactly as in any terminal. Hooks are installed on the first prompt so
//! they run outside the user's configuration (first in precmd, last in preexec).
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

pub const ZSHENV: &str = r#"# Ember: restore the user's ZDOTDIR, then load their own .zshenv
if [[ -n ${EMBER_USER_ZDOTDIR+x} ]]; then
  ZDOTDIR=$EMBER_USER_ZDOTDIR
else
  unset ZDOTDIR
fi
unset EMBER_USER_ZDOTDIR
[[ -r ${ZDOTDIR:-$HOME}/.zshenv ]] && builtin source ${ZDOTDIR:-$HOME}/.zshenv
[[ -o interactive && -r $EMBER_SHELL_DIR/ember.zsh ]] && builtin source $EMBER_SHELL_DIR/ember.zsh
"#;

pub const EMBER_ZSH: &str = r#"# Ember shell integration: invisible in-band markers (OSC 7777). No prompt changes.
typeset -g _ember_cmd=0 _ember_loop=0
_ember_clean() { REPLY=${1//$'\a'/}; REPLY=${REPLY//$'\e'/}; REPLY=${REPLY//$'\x1f'/ }; }
_ember_preexec() {
  local typed=$1 full=$3 head body det loop=0
  local -a words; words=(${(z)full}); head=${words[1]}
  [[ $head == find && -n $_ember_own_find ]] || { (( $+functions[$head] )) && body=${functions[$head]}; }
  det=${body:-$full}
  if [[ $det =~ '(^|[[:space:];])find[[:space:]].*-exec[[:space:]]' || $det =~ '(^|[[:space:];])(for|while)[[:space:]]' ]]; then loop=1; fi
  local t f p d
  _ember_clean "$typed"; t=$REPLY; _ember_clean "$full"; f=$REPLY; _ember_clean "$PWD"; p=$REPLY; _ember_clean "$body"; d=$REPLY
  builtin print -rn -- $'\e]7777;P;'"$t"$'\x1f'"$f"$'\x1f'"$p"$'\x1f'"$loop"$'\x1f'"$d"$'\a'
  _ember_cmd=1
  if (( loop )) && [[ -t 2 ]] && ! (( $+functions[TRAPDEBUG] )); then
    _ember_loop=1
    builtin trap _ember_dbg DEBUG
  fi
}
# Each command a loop runs, with its simple parameters expanded (never command substitutions).
_ember_dbg() {
  local w c=${ZSH_DEBUG_CMD%%$'\n'*}
  local -a o
  for w in ${(z)c}; do
    if [[ $w == *'$('* || $w == *'`'* ]]; then o+=($w); else o+=(${(Q)${(e)w}}); fi
  done
  _ember_clean "${o[*]}"
  builtin print -rn -u2 -- $'\e]7777;T;'"$REPLY"$'\a'
  return 0
}
_ember_precmd() {
  local rc=$?
  if (( _ember_loop )); then
    builtin trap - DEBUG
    _ember_loop=0
  fi
  if (( _ember_cmd )); then
    builtin print -rn -- $'\e]7777;D;'"$rc"$'\a'
    _ember_cmd=0
  fi
  return $rc
}
# find -exec items: a marker before each per-item -exec, only while a recorded loop writes to the terminal
if ! (( $+functions[find] || $+aliases[find] )); then
  typeset -g _ember_own_find=1
  find() {
    if (( _ember_loop )) && [[ -t 1 ]]; then
      local -a a; local i=1 j x
      while (( i <= $# )); do
        x=${@[i]}
        if [[ $x == -exec || $x == -execdir ]]; then
          j=$((i+1))
          while (( j <= $# )) && [[ ${@[j]} != ';' && ${@[j]} != '+' ]]; do (( j++ )); done
          [[ ${@[j]} == ';' ]] && a+=(-exec printf '\033]7777;I;%s\007' '{}' ';')
        fi
        a+=("$x"); (( i++ ))
      done
      command find "${a[@]}"
    else
      command find "$@"
    fi
  }
fi
_ember_install() {
  precmd_functions=(${precmd_functions:#_ember_install})
  precmd_functions=(_ember_precmd ${precmd_functions:#_ember_precmd})
  preexec_functions=(${preexec_functions:#_ember_preexec} _ember_preexec)
}
autoload -Uz add-zsh-hook 2>/dev/null
precmd_functions+=(_ember_install)
"#;

/// Writes the integration files into `<data>/shell` and returns that folder.
pub fn install(data_dir: &Path) -> Result<PathBuf> {
    let dir = data_dir.join("shell");
    fs::create_dir_all(&dir)?;
    fs::write(dir.join(".zshenv"), ZSHENV)?;
    fs::write(dir.join("ember.zsh"), EMBER_ZSH)?;
    Ok(dir)
}

/// Environment for a pane's zsh.
pub fn env(shell_dir: &Path) -> Vec<(String, String)> {
    let mut v = vec![
        ("EMBER_SHELL_DIR".into(), shell_dir.display().to_string()),
        ("ZDOTDIR".into(), shell_dir.display().to_string()),
        ("TERM".into(), "xterm-256color".into()),
        ("COLORTERM".into(), "truecolor".into()),
        ("TERM_PROGRAM".into(), "Ember".into()),
        ("EMBER".into(), "1".into()),
    ];
    if let Ok(z) = std::env::var("ZDOTDIR") {
        v.push(("EMBER_USER_ZDOTDIR".into(), z));
    }
    v
}
