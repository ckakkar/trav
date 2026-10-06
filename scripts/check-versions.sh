#!/usr/bin/env bash
# Fails if the workspace (which the desktop crate inherits), Tauri config and npm package disagree on the version.
set -euo pipefail
cd "$(dirname "$0")/.."
ws=$(sed -n '/^\[workspace.package\]/,/^\[/{s/^version *= *"\(.*\)"/\1/p;}' Cargo.toml)
conf=$(node -p "require('./trav-gui/src-tauri/tauri.conf.json').version")
npm=$(node -p "require('./trav-gui/package.json').version")
tag=${GITHUB_REF_NAME:-}
echo "workspace=$ws tauri.conf=$conf package.json=$npm ${tag:+tag=$tag}"
grep -q '^version.workspace = true' trav-gui/src-tauri/Cargo.toml || { echo "trav-desktop must inherit the workspace version" >&2; exit 1; }
[[ "$ws" == "$conf" && "$ws" == "$npm" ]] || { echo "version mismatch" >&2; exit 1; }
if [[ "$tag" == v* && "${tag#v}" != "$ws" ]]; then echo "tag $tag does not match version $ws" >&2; exit 1; fi
