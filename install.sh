#!/bin/sh
set -eu

usage() {
    echo 'Usage: ./install.sh [bash|zsh|fish]'
    echo 'Install FileTrail with Cargo and configure Tab completion (defaults to $SHELL).'
    echo 'CARGO_INSTALL_ROOT overrides the installation root; otherwise CARGO_HOME or ~/.cargo is used.'
}

if [ "$#" -gt 1 ]; then
    usage >&2
    exit 2
fi
case "${1-}" in
    -h|--help) usage; exit 0 ;;
esac
filetrail_shell=${SHELL-}
filetrail_shell=${1:-${filetrail_shell##*/}}
case "$filetrail_shell" in
    bash|zsh|fish) ;;
    *) echo 'Specify bash, zsh, or fish: ./install.sh zsh' >&2; exit 2 ;;
esac

filetrail_project=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
filetrail_install_root=${CARGO_INSTALL_ROOT:-${CARGO_HOME:-"$HOME/.cargo"}}
mkdir -p "$filetrail_install_root"
filetrail_install_root=$(CDPATH= cd -- "$filetrail_install_root" && pwd)
cd "$filetrail_project"
cargo install --path . --locked --root "$filetrail_install_root"
"$filetrail_install_root/bin/filetrail" completions "$filetrail_shell" --install
