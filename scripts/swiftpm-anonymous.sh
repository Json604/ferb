# Source this before any `swift build`, `swift test` or `swift package` (zsh or bash): SwiftPM fetches its public
# dependencies (Sparkle) with git, and git's keychain credential helper would ask for the owner's github.com login.
# FERB's build never touches the owner's keychain, so git runs here with no system or user config, no credential
# helper and no prompt: the fetch is anonymous.
export GIT_TERMINAL_PROMPT=0
export GIT_CONFIG_NOSYSTEM=1
export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_COUNT=1
export GIT_CONFIG_KEY_0=credential.helper
export GIT_CONFIG_VALUE_0=
export GIT_ASKPASS=/usr/bin/true
export SSH_ASKPASS=/usr/bin/true
