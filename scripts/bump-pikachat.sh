#!/usr/bin/env bash
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
VERSION_FILE="$ROOT/VERSION"
ROOT_VERSION="$("$ROOT/scripts/version-read" --name)"

if [ $# -gt 1 ]; then
  echo "Usage: $0 [<version>]" >&2
  echo "Example: $0 1.1.0" >&2
  echo "When omitted, the repo-root VERSION value is used." >&2
  echo "Set PIKACHAT_ALLOW_VERSION_DRIFT=1 to intentionally release pikachat at a different version." >&2
  exit 1
fi

branch="$(git rev-parse --abbrev-ref HEAD)"
if [ "$branch" != "master" ]; then
  echo "error: pikachat releases must be tagged from master (currently on $branch)" >&2
  exit 1
fi

if [ -n "$(git status --porcelain)" ]; then
  echo "error: git working tree is dirty" >&2
  git status --short >&2
  exit 1
fi

if [ $# -eq 1 ]; then
  VERSION="$1"
  if [ "$VERSION" = "$ROOT_VERSION" ]; then
    INCLUDE_ROOT_VERSION=1
  elif [ "${PIKACHAT_ALLOW_VERSION_DRIFT:-0}" = "1" ]; then
    echo "warning: requested pikachat version ($VERSION) differs from VERSION ($ROOT_VERSION)" >&2
    echo "warning: continuing because PIKACHAT_ALLOW_VERSION_DRIFT=1 is set" >&2
    INCLUDE_ROOT_VERSION=0
  else
    echo "error: requested pikachat version ($VERSION) differs from VERSION ($ROOT_VERSION)" >&2
    echo "hint: run \`just release-commit $VERSION\` for a coordinated release," >&2
    echo "      or set PIKACHAT_ALLOW_VERSION_DRIFT=1 for an intentional pikachat-only release" >&2
    exit 1
  fi
else
  VERSION="$ROOT_VERSION"
  INCLUDE_ROOT_VERSION=1
fi

TAG="pikachat-v${VERSION}"
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null 2>&1; then
  echo "error: tag already exists: $TAG" >&2
  exit 1
fi

"$ROOT/scripts/sync-pikachat-version" "$VERSION"

# Stage and commit
git add "$ROOT/cli/Cargo.toml" \
  "$ROOT/pikachat-openclaw/openclaw/extensions/pikachat-openclaw/package.json" \
  "$ROOT/Cargo.lock"
if [ "$INCLUDE_ROOT_VERSION" -eq 1 ]; then
  git add "$VERSION_FILE"
fi
git commit -m "release: pikachat v${VERSION}"

# Tag
git tag "$TAG"

echo ""
echo "Done. To release:"
echo "  git push origin master $TAG"
