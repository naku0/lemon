#!/bin/sh
set -eu

repository_root=$(git rev-parse --show-toplevel)
cd "$repository_root"
git config core.hooksPath .githooks

echo "LEMON Git hooks enabled from .githooks"
