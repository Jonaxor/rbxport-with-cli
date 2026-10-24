#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$REPO_ROOT"

if [[ $# -eq 0 ]]; then
  exec nix --extra-experimental-features 'nix-command flakes' develop . -c cargo run -p rbxport --bin rbxport-cli -- --help
fi

exec nix --extra-experimental-features 'nix-command flakes' develop . -c cargo run -p rbxport --bin rbxport-cli -- "$@"
