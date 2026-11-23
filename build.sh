#!/bin/zsh
# Build the current Rust implementation without replacing the live app.
set -euo pipefail
exec "${0:A:h}/rust/build-candidate.sh" "$@"
