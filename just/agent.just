set working-directory := '..'

# Run the HTTP agent ensure demo (`pikachat agent new --nsec ...`).
agent-incus *ARGS="":
    ./scripts/demo-agent-incus.sh {{ ARGS }}

# Ensure/reuse an Incus-backed agent, send one message, then optionally listen for reply.
agent-incus-chat MESSAGE="CLI Incus demo check: reply with ACK and one short sentence." *ARGS="":
    ./scripts/agent-demo-incus.sh "{{ MESSAGE }}" {{ ARGS }}
