#!/usr/bin/env bash
# Run cargo, falling back to a nix-provided toolchain when cargo is not on PATH.
set -euo pipefail

if command -v cargo >/dev/null 2>&1; then
  exec cargo "$@"
fi

export XDG_CACHE_HOME="${XDG_CACHE_HOME:-/tmp/nix-cache}"
export CARGO_HOME="${CARGO_HOME:-/tmp/cargo-home}"
exec nix shell nixpkgs#cargo nixpkgs#rustc -c cargo "$@"
