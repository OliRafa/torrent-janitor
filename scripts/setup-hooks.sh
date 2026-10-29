#!/usr/bin/env bash
# Point git at the hooks kept in this repository, so everyone runs the same ones.
set -euo pipefail

cd "$(dirname "$0")/.."
git config core.hooksPath hooks
chmod +x hooks/* scripts/*.sh
echo "Git hooks enabled. Every commit and push now runs ./scripts/ci.sh"
