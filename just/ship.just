# Sync the shared release version across app + pikachat/OpenClaw manifests.
release-bump VERSION:
    set -euo pipefail; \
    release_version="{{ VERSION }}"; \
    case "$release_version" in VERSION=*) release_version="${release_version#VERSION=}";; esac; \
    ./scripts/set-release-version "$release_version"

# Create a single coordinated release version commit without tagging.
release-commit VERSION:
    set -euo pipefail; \
    release_version="{{ VERSION }}"; \
    case "$release_version" in VERSION=*) release_version="${release_version#VERSION=}";; esac; \
    branch="$(git rev-parse --abbrev-ref HEAD)"; \
    if [ "$branch" != "master" ]; then \
      echo "error: release commits must be created from master (currently on $branch)"; \
      exit 1; \
    fi; \
    if [ -n "$(git status --porcelain)" ]; then \
      echo "error: git working tree is dirty"; \
      git status --short; \
      exit 1; \
    fi; \
    ./scripts/set-release-version "$release_version"; \
    git add VERSION ios/project.yml cli/Cargo.toml pikachat-openclaw/openclaw/extensions/pikachat-openclaw/package.json Cargo.lock; \
    git commit -m "release: bump to $release_version"

# Create + push version tag (pika/vX.Y.Z) after validating VERSION and clean tree.
release VERSION:
    set -euo pipefail; \
    branch="$(git rev-parse --abbrev-ref HEAD)"; \
    if [ "$branch" != "master" ]; then \
      echo "error: releases must be tagged from master (currently on $branch)"; \
      exit 1; \
    fi; \
    release_version="{{ VERSION }}"; \
    case "$release_version" in VERSION=*) release_version="${release_version#VERSION=}";; esac; \
    current_version="$(./scripts/version-read --name)"; \
    if [ "$release_version" != "$current_version" ]; then \
      echo "error: release version ($release_version) does not match file VERSION ($current_version)"; \
      exit 1; \
    fi; \
    if [ -n "$(git status --porcelain)" ]; then \
      echo "error: git working tree is dirty"; \
      git status --short; \
      exit 1; \
    fi; \
    tag="pika/v$release_version"; \
    if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then \
      echo "error: tag already exists: $tag"; \
      exit 1; \
    fi; \
    git tag "$tag"; \
    git push origin "$tag"; \
    echo "ok: pushed $tag"

# Encrypt Zapstore signing value to `secrets/zapstore-signing.env.age`.
zapstore-encrypt-signing:
    ./scripts/encrypt-zapstore-signing

# Check Zapstore publish inputs for a local APK without publishing events.
zapstore-check APK:
    zsp publish --check "{{ APK }}" -r https://github.com/sledtools/pika

# Publish a local APK artifact to Zapstore.
zapstore-publish APK:
    ./scripts/zapstore-publish "{{ APK }}" https://github.com/sledtools/pika
