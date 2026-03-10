set working-directory := '..'

# Run the HTTP agent ensure demo (`pikachat agent new --nsec ...`).
agent-microvm *ARGS="":
    ./scripts/demo-agent-microvm.sh {{ ARGS }}

# Run the HTTP agent ensure demo with ACP microVM guest bootstrap.
agent-microvm-acp *ARGS="":
    PIKA_AGENT_MICROVM_BACKEND=acp ./scripts/demo-agent-microvm.sh {{ ARGS }}

# Run the HTTP agent ensure demo for a Pi ACP-backed guest.
agent-pi-ensure *ARGS="":
    PIKA_AGENT_MICROVM_KIND=pi ./scripts/demo-agent-microvm.sh {{ ARGS }}

# Run the HTTP agent ensure demo for an OpenClaw guest over the daemon protocol.
agent-claw-ensure *ARGS="":
    PIKA_AGENT_MICROVM_KIND=openclaw ./scripts/demo-agent-microvm.sh {{ ARGS }}

# Ensure/reuse agent, send one message, then optionally listen for reply.
agent-microvm-chat MESSAGE="hello from pikachat cli" *ARGS="":
    ./scripts/pikachat-cli.sh agent chat "{{ MESSAGE }}" {{ ARGS }}

# Tail local pika-server logs from the pikahut backend state dir.
agent-microvm-server-logs STATE_DIR=".pikahut":
    cargo run -p pikahut -- logs --state-dir {{ STATE_DIR }} --follow --component server

# Tail remote vm-spawner logs on the pika-build microVM host.
agent-microvm-vmspawner-logs:
    nix develop .#infra -c just -f infra/justfile build-vmspawner-logs

# Tail the guest agent log for a specific VM on the pika-build microVM host.
agent-microvm-guest-logs VM_ID:
    nix develop .#infra -c just -f infra/justfile build-guest-logs {{ VM_ID }}

# Reset the current test account's VM on pika-build, recover/create via pika-server, then chat.
agent-demo MESSAGE="CLI demo check: reply with ACK and one short sentence.":
    ./scripts/agent-demo.sh "{{ MESSAGE }}"

# Reset/create/recover the current test account's VM, then chat through an ACP-backed guest daemon.
agent-demo-acp MESSAGE="CLI demo check: reply with ACK and one short sentence.":
    PIKA_AGENT_MICROVM_BACKEND=acp ./scripts/agent-demo.sh "{{ MESSAGE }}"

# Ensure/reuse a Pi ACP guest, then chat.
agent-pi MESSAGE="CLI demo check: reply with ACK and one short sentence.":
    PIKA_AGENT_MICROVM_KIND=pi ./scripts/agent-demo.sh "{{ MESSAGE }}"

# Ensure/reuse an OpenClaw guest, then chat.
agent-claw MESSAGE="CLI demo check: reply with ACK and one short sentence.":
    PIKA_AGENT_MICROVM_KIND=openclaw ./scripts/agent-demo.sh "{{ MESSAGE }}"

# Open local port-forward to remote vm-spawner (`http://127.0.0.1:8080`).
agent-microvm-tunnel:
    nix develop .#infra -c just -f infra/justfile build-vmspawner-tunnel
