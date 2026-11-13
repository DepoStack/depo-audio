#!/usr/bin/env bash
# The one local CI command (platform plan section 10.2): `npm run ci:local`.
# Runs the same checks as .github/workflows/ci.yml, in order, and stops on the
# first failure. DepoAudio is public, so the hosted workflow stays on
# GitHub-hosted runners; this script is the same gate for a laptop or an
# offline run.
#   1. npm ci, npm audit --audit-level=high
#   2. zero learned-model check, JavaScript notices, release contract
#   3. lint, formatting, design tokens in sync
#   4. unit and characterization tests, build, bundle size budget
#   5. cargo-deny advisories, then bans/licenses/sources (cargo-deny required)
#   6. cargo clippy -D warnings, cargo fmt --check, cargo test (Tauri system
#      libraries and an ffmpeg/ffprobe on PATH required)
#   7. gitleaks over the full local history, as the hosted secret-scan job
#      does (skipped with a message if not installed; CI builds v8.30.1)
#   8. evidence file .ci-evidence/<time>-<commit>.json (git-ignored)
# Needs no secret. Network: the npm registry, crates.io and the RustSec
# advisory database. The release workflows are not run here.
set -euo pipefail
cd "$(dirname "$0")/.."
START=$(date -u +%Y-%m-%dT%H:%M:%SZ)
STEPS=""
STAGED_SIDECARS=()

cleanup() {
  for file in "${STAGED_SIDECARS[@]+"${STAGED_SIDECARS[@]}"}"; do rm -f "$file"; done
  rmdir src-tauri/binaries 2>/dev/null || true
}
trap cleanup EXIT

finish() {
  mkdir -p .ci-evidence
  local commit branch file
  commit=$(git rev-parse HEAD 2>/dev/null || echo unknown)
  branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo unknown)
  file=".ci-evidence/$(date -u +%Y%m%dT%H%M%SZ)-${commit:0:12}.json"
  printf '{"repository":"depo-audio","commit":"%s","branch":"%s","result":"%s","os":"%s","node":"%s","npm":"%s","rustc":"%s","startedAt":"%s","endedAt":"%s","steps":[%s]}\n' \
    "$commit" "$branch" "$1" "$(uname -sm)" "$(node -v)" "$(npm -v)" \
    "$(rustc --version 2>/dev/null || echo none)" "$START" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$STEPS" >"$file"
  echo "Evidence: $file"
}

step() { # name command...
  local name="$1" t0
  shift
  printf '\n== %s\n' "$name"
  t0=$(date +%s)
  if "$@"; then
    STEPS="${STEPS:+$STEPS,}{\"name\":\"$name\",\"status\":\"passed\",\"seconds\":$(($(date +%s) - t0))}"
  else
    STEPS="${STEPS:+$STEPS,}{\"name\":\"$name\",\"status\":\"failed\",\"seconds\":$(($(date +%s) - t0))}"
    finish failed
    exit 1
  fi
}

skip() {
  printf '\n== %s: skipped (%s)\n' "$1" "$2"
  STEPS="${STEPS:+$STEPS,}{\"name\":\"$1\",\"status\":\"skipped\",\"reason\":\"$2\"}"
}

require() { # tool hint
  command -v "$1" >/dev/null || {
    echo "ERROR: $1 is required ($2)"
    STEPS="${STEPS:+$STEPS,}{\"name\":\"$1\",\"status\":\"failed\",\"reason\":\"not installed\"}"
    finish failed
    exit 1
  }
}

tokens_in_sync() {
  npm run tokens && git diff --exit-code src/styles/tokens.css
}

# tauri-build checks that the configured externalBin sidecars exist for the
# host target. Stage the system FFmpeg under the target-triple names when they
# are missing, as ci.yml does, and remove only the files staged here.
stage_sidecars() {
  local triple tool target
  triple=$(rustc -vV | sed -n 's/^host: //p')
  mkdir -p src-tauri/binaries
  for tool in ffmpeg ffprobe; do
    target="src-tauri/binaries/$tool-$triple"
    if [ ! -e "$target" ]; then
      cp "$(command -v "$tool")" "$target"
      STAGED_SIDECARS+=("$target")
    fi
  done
}

step install npm ci
step npm-audit npm audit --audit-level=high
step models-check npm run models:check
step notices npm run notices:self-test
step notices-check npm run notices:check
step release-contract npm run release:check
step lint npm run lint
step format npm run format:check
step tokens tokens_in_sync
step test npm test
step build npm run build
step size npm run size:check

require cargo-deny 'cargo install --locked cargo-deny, or the 0.20.2 release binary CI uses'
step cargo-deny-advisories cargo deny --manifest-path src-tauri/Cargo.toml --locked --all-features check advisories
step cargo-deny-policy cargo deny --manifest-path src-tauri/Cargo.toml --locked --all-features check bans licenses sources

require ffmpeg 'tauri-build needs host sidecars; release builds use the pinned sidecars instead'
require ffprobe 'tauri-build needs host sidecars'
stage_sidecars
step clippy cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
step rustfmt cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
step cargo-test cargo test --locked --manifest-path src-tauri/Cargo.toml

if command -v gitleaks >/dev/null; then
  step gitleaks gitleaks git --no-banner --redact --exit-code 1 --log-opts=--all .
else
  skip gitleaks 'not installed'
fi

finish passed
echo 'ci:local passed'
