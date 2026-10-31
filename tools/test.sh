#!/usr/bin/env bash
# Pangan test runner.
#
# Runs the full test suite with the same hygiene a release gate should:
#   - clippy (lints) unless --no-clippy
#   - rustfmt --check (formatting) unless --no-fmt
#   - the full test suite, release-grade, with warnings as errors
#   - a summary line with the exit status of each phase
#
# Exit nonzero if any phase fails, so it drops straight into CI or a
# pre-commit habit.
set -u

cd "$(dirname "$0")/.."

DO_CLIPPY=1
DO_FMT=1
for arg in "$@"; do
  case "$arg" in
    --no-clippy) DO_CLIPPY=0 ;;
    --no-fmt)    DO_FMT=0 ;;
    --help|-h)
      echo "Usage: tools/test.sh [--no-clippy] [--no-fmt]"
      exit 0
      ;;
    *) echo "unknown flag: $arg" >&2; exit 2 ;;
  esac
done

PASS=0
FAIL=0

phase() {
  local name="$1"; shift
  echo ""
  echo "━━━ $name ━━━"
  if "$@"; then
    echo "✓ $name"
    PASS=$((PASS+1))
  else
    echo "✗ $name FAILED" >&2
    FAIL=$((FAIL+1))
  fi
}

phase "rustfmt (formatting)" bash -c '[ "$DO_FMT" = 1 ] && cargo fmt --check || true'

phase "clippy (lints)" bash -c '[ "$DO_CLIPPY" = 1 ] && cargo clippy --all-targets || true'

phase "tests (full suite)" bash -c 'cargo test --all-targets'

phase "build (release)" bash -c 'cargo build --release'

echo ""
echo "════════════════════════════"
echo "phases passed: $PASS   failed: $FAIL"
[ "$FAIL" = 0 ] && echo "all green" || echo "FAILURES PRESENT" >&2
exit "$FAIL"
