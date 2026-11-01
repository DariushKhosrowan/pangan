#!/usr/bin/env bash
# Pangan UI immutability guard.
#
# The desktop graphical interface (everything under ui/) is treated as
# immutable for the mobile-platforms work. This check is the automated
# enforcement: it fails if any protected UI file is added, deleted, renamed,
# or has its content changed relative to the recorded baseline commit, and if
# any untracked file appears under ui/.
#
# Usage: tools/check_ui_immutable.sh
# Exit 0 when the UI tree is untouched; 1 otherwise.
set -u

cd "$(dirname "$0")/.."

# Baseline commit captured at the start of the mobile-platforms work
# (2026-11-01, HEAD was 21d3159). All protected UI files are compared against
# the copies in this commit.
BASELINE="21d315915641f477ac99e564e524717b07f7ad90"

FAIL=0

echo "── protected UI baseline: $BASELINE"

# 1. Tracked-file changes (content edit, rename, delete) under ui/.
CHANGES=$(git diff --name-status "$BASELINE" -- ui/)
if [ -n "$CHANGES" ]; then
  echo "✗ PROTECTED UI changed vs baseline:"
  echo "$CHANGES"
  FAIL=1
else
  echo "✓ no tracked changes under ui/ since baseline"
fi

# 2. Untracked additions under ui/ (new files would also be a change).
UNTRACKED=$(git status --porcelain ui/ | grep -E '^\?\?' || true)
if [ -n "$UNTRACKED" ]; then
  echo "✗ untracked files added under ui/:"
  echo "$UNTRACKED"
  FAIL=1
else
  echo "✓ no untracked files under ui/"
fi

# 3. Byte-for-byte hash comparison against the baseline tree, to be robust
#    even if git metadata is unusual.
MISMATCH=0
while IFS= read -r line; do
  expected_hash=${line%%  *}
  f=${line#*  }
  actual=$(git show "$BASELINE:$f" 2>/dev/null | sha256sum | cut -d' ' -f1)
  if [ "$actual" != "$expected_hash" ]; then
    echo "✗ content mismatch vs baseline: $f"
    MISMATCH=1
  fi
done < <(git ls-tree -r --name-only "$BASELINE" ui/ | while IFS= read -r f; do
  echo "$(git show "$BASELINE:$f" | sha256sum | cut -d' ' -f1)  $f"
done)
if [ "$MISMATCH" = 0 ]; then
  echo "✓ all baseline UI files byte-identical"
else
  FAIL=1
fi

if [ "$FAIL" = 0 ]; then
  echo "✓ UI immutable — OK"
else
  echo "✗ UI PROTECTION FAILED" >&2
fi
exit "$FAIL"
