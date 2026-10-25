# FERB.app's parts per architecture (L34), sourced by build-app.sh (zsh). Nothing here builds, signs or downloads.

# _ferb_bytes PATH…: the total size in bytes of the regular files under each PATH (0 when there are none)
_ferb_bytes() {
  local p
  for p in "$@"; do
    if [[ -e $p ]]; then find "$p" -type f -print0; fi
  done | xargs -0 stat -f %z | awk '{ s += $1 } END { printf "%.0f\n", s }'
}

# ferb_size_report APP: FERB.app's size by part in MB (10^6 bytes), the largest first (L34's measurement)
ferb_size_report() {
  local app=${1:A}
  local c="$app/Contents" py="$app/Contents/Resources/python"
  local std="$py/lib/python3.12" site="$py/lib/python3.12/site-packages"
  local total codex claude site_all std_all libpy notch helpers arch
  total=$(_ferb_bytes "$app")
  codex=$(_ferb_bytes "$c/Resources/node")
  claude=$(_ferb_bytes "$site/claude_agent_sdk/_bundled")
  site_all=$(_ferb_bytes "$site")
  std_all=$(_ferb_bytes "$std")
  libpy=$(_ferb_bytes "$py/lib/libpython3.12.dylib")
  notch=$(_ferb_bytes "$c/MacOS")
  helpers=$(_ferb_bytes "$c/Helpers")
  arch=$(lipo -archs "$c/MacOS/FERBNotch" 2>/dev/null || print unknown)
  local -a rows
  rows=("$codex|Codex (Resources/node)"
        "$claude|Claude Code (claude_agent_sdk/_bundled)"
        "$(( site_all - claude ))|Python packages (site-packages without Claude Code)"
        "$(( std_all - site_all ))|Python standard library"
        "$libpy|libpython"
        "$notch|Notch app (MacOS)"
        "$helpers|Helpers (agent, voice, secrets)"
        "$(( total - codex - std_all - libpy - notch - helpers ))|Everything else")
  printf 'FERB.app (%s): %.1f MB\n' "$arch" $(( total / 1e6 ))
  local bytes name
  print -l -- $rows | sort -t'|' -k1,1 -rn | while IFS='|' read -r bytes name; do
    printf '%8.1f MB  %s\n' $(( bytes / 1e6 )) "$name"
  done
}
