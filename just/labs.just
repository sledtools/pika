set working-directory := '..'

# Ensure an Android target is booted and ready (without building/installing app).
android-device-start *ARGS:
    just rmp devices start android {{ ARGS }}

# Boot Android target and open app with agent-device.
android-agent-open APP="com.justinmoon.pika.dev" *ARGS="":
    just android-device-start {{ ARGS }}
    ./tools/agent-device --platform android open {{ APP }}

# Manual-only selector contracts (never required in CI lanes).
[private]
integration-manual:
    cargo test -p pikahut --test integration_manual manual_interop_rust_runbook_contract -- --ignored --nocapture
    cargo test -p pikahut --test integration_manual manual_primal_lab_runbook_contract -- --ignored --nocapture

# Local Primal interop lab: dedicated simulator + local relay + event tap logs.
primal-ios-lab:
    ./tools/primal-ios-interop-lab run

# Apply debug logging patch in local Primal checkout (~/code/primal-ios-app by default).
[private]
primal-ios-lab-patch-primal:
    ./tools/primal-ios-interop-lab patch-primal

# Capture current lab simulator as a reusable seeded snapshot.
[private]
primal-ios-lab-seed-capture:
    ./tools/primal-ios-interop-lab seed-capture

# Reset the lab simulator from the saved seed snapshot.
[private]
primal-ios-lab-seed-reset:
    ./tools/primal-ios-interop-lab seed-reset

# Print Pika's latest nostr-connect debug snapshot and a decode helper command.
[private]
primal-ios-lab-dump-debug:
    ./tools/primal-ios-interop-lab dump-debug

# Interop baseline: local Rust bot. Requires ~/code/marmot-interop-lab-rust.
interop-rust-baseline:
    cargo test -p pikahut --test integration_deterministic interop_rust_baseline -- --ignored --nocapture

# Interactive interop test (manual send/receive with local bot).
[private]
interop-rust-manual:
    cargo test -p pikahut --test integration_manual manual_interop_rust_runbook_contract -- --ignored --nocapture

# Optional: device automation (npx). Not required for building.
[private]
device:
    ./tools/agent-device --help

# Show Android manual QA instructions.
[private]
android-manual-qa:
    @echo "Manual QA prompt: prompts/android-agent-device-manual-qa.md"
    @echo "Tip: run `npx --yes agent-device --platform android open org.pikachat.pika.dev` then follow the prompt."

# Show iOS manual QA instructions.
[private]
ios-manual-qa:
    @echo "Manual QA prompt: prompts/ios-agent-device-manual-qa.md"
    @echo "Tip: run `./tools/agent-device --platform ios open org.pikachat.pika.dev` then follow the prompt."
