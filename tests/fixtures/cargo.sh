#!/bin/sh
# Cargo stand-in for install.sh integration tests; never modifies real user data.
set -eu

if [ "$FILETRAIL_TEST_CARGO_EXIT" != 0 ]; then
    exit "$FILETRAIL_TEST_CARGO_EXIT"
fi

[ "$1 $2 $3 $4 $5" = 'install --path . --locked --root' ]
mkdir -p "$6/bin"
cp "$FILETRAIL_TEST_BINARY" "$6/bin/filetrail"
