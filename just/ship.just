set working-directory := '..'

# Sync the shared release version across app + pikachat/OpenClaw manifests.
release-bump VERSION:
    ./scripts/release-version release-bump "{{ VERSION }}"

# Create a single coordinated release version commit without tagging.
release-commit VERSION:
    ./scripts/release-version release-commit "{{ VERSION }}"

# Create + push version tag (pika/vX.Y.Z) after validating VERSION and clean tree.
release VERSION:
    ./scripts/release-version release "{{ VERSION }}"

# Encrypt Zapstore signing value to `secrets/zapstore-signing.sops.yaml`.
zapstore-encrypt-signing:
    ./scripts/encrypt-zapstore-signing

# Check Zapstore publish inputs for a local APK without publishing events.
zapstore-check APK:
    ./scripts/zapstore-publish --check "{{ APK }}" https://github.com/sledtools/pika

# Publish a local APK artifact to Zapstore.
zapstore-publish APK:
    ./scripts/zapstore-publish "{{ APK }}" https://github.com/sledtools/pika
