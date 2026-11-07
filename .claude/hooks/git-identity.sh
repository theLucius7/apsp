#!/bin/bash
# Commit as the repository owner in cloud sessions instead of the default agent identity.
set -euo pipefail

# Local checkouts keep the developer's own git configuration.
if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

cd "${CLAUDE_PROJECT_DIR:-.}"
git config user.name "Lucius7 Nya"
git config user.email "i@lucius7.dev"
# The environment's signing key belongs to the agent, not to this identity.
git config commit.gpgsign false
