#!/usr/bin/env bash
set -euo pipefail

if [ $# -ne 1 ]; then
  echo "Usage: $0 <version>" >&2
  echo "Example: $0 0.3.1" >&2
  exit 1
fi

VERSION="$1"
TAG="pikachat-v${VERSION}"
ROOT="$(git rev-parse --show-toplevel)"

CARGO_TOML="$ROOT/cli/Cargo.toml"
PACKAGE_JSON="$ROOT/pikachat-openclaw/openclaw/extensions/pikachat-openclaw/package.json"

# Bump Cargo.toml
sed "s/^version = \".*\"/version = \"${VERSION}\"/" "$CARGO_TOML" > "$CARGO_TOML.tmp" && mv "$CARGO_TOML.tmp" "$CARGO_TOML"

# Bump package.json
cd "$ROOT/pikachat-openclaw/openclaw/extensions/pikachat-openclaw"
npm version "$VERSION" --no-git-tag-version --allow-same-version

# Update Cargo.lock
cd "$ROOT"
cargo check -p pikachat --quiet

# Stage and commit
git add "$CARGO_TOML" "$PACKAGE_JSON" "$ROOT/Cargo.lock"
git commit -m "release: pikachat v${VERSION}"

# Tag
git tag "$TAG"

echo ""
echo "Done. To release:"
echo "  git push origin master $TAG"
