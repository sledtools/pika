# Audio Calls E2E Plan (Real MOQ + OpenClaw + STT/TTS)

Date: 2026-02-14

Goal: full voice comms: real client ↔ `moq.justinmoon.com` ↔ OpenClaw bot (openclaw-marmot sidecar) ↔ STT/LLM/TTS ↔ audio response back to caller.

## Context (identifiers)

- MOQ relay URL (v0 auth root): `https://moq.justinmoon.com/anon`
- Bot npub: `npub1rtrxx9eyvag0ap3v73c4dvsqq5d2yxwe5d72qxrfpwe5svr96wuqed4p38`
- Allowed callers (streambot `marmotd --allow-pubkey`):
  - Justin (real): `npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y` (hex `11b9a894...`)
  - Test key: `npub1y2z0c7un9dwmhk4zrpw8df8p0gh0j2x54qhznwqjnp452ju4078srmwp70` (hex `2284fc7b...`)
  - Paul: `npub1qjzr79nqfwducv4adfyg0zwl9qg4jvq9sspc8czsc0ekr8xm6ttsth5h4k` (hex `04843f16...`)

Status check (streambot, verified 2026-02-14):
- `/run/secrets/openclaw/openai_api_key` exists.
- `openclaw-gateway.service` is active and starts `marmotd` with `--allow-pubkey` for:
  - Justin (hex) `11b9a894813efe60d39f8621ae9dc4c6d26de4732411c1cdf4bb15e88898a19c`
  - Test (hex) `2284fc7b932b5dbbdaa2185c76a4e17a2ef928d4a82e29b812986b454b957f8f`
  - Paul (hex) `04843f16604b9bcc32bd6a488789df2811593005840383e050c3f3619cdbd2d7`
- `openclaw-gateway` loads `/run/secrets/rendered/openclaw-env` (contains `OPENAI_API_KEY` + `ANTHROPIC_API_KEY`).

Important: streambot currently logs an invalid config error for `plugins.entries.marmot.config` (missing required `relays`) and the deployed marmot extension does not include the Phase-8 voice wiring (`send_audio_response`). Step 2 below makes that explicit.

## Step 0 — Infra hygiene (stash)

Why: avoid surprise deploy diffs; keep future deploys explainable.

Actions:
- On laptop: `cd ~/code/infra`
- Inspect: `git status`, `git log -1`, `git stash list`
- Note: as of 2026-02-14 local `~/code/infra` had `stash@{0}: On master: wip: local infra changes (pre-deploy)`; treat as unknown until reviewed.
- For the “unknown” stash: `git stash show -p stash@{N}`
- Decide:
  - If relevant: apply, review, commit with a clear message
  - If irrelevant: drop stash (or keep it explicitly labeled)

Findings (laptop, observed 2026-02-14):
- `~/code/infra` working tree: clean.
- `git log -1`: `dd7862a docs(sops): document recipients and local editing`.
- Stash `stash@{0}` contents:
  - Updates `flake.lock` pin for `openclawMarmotSrc` from `410590ec...` to `b24b4a71...`.
  - Removes `.gitignore` entry for `secrets/openclaw-local.yaml` (risky; could allow accidental commit).
  - Deletes `justfile` recipes `openclaw-local-init`, `openclaw-local-edit`, `openclaw-local-upload` (local convenience for encrypted key handling).
- Decision: keep the stash for now (do not apply) until we know whether the `openclaw-marmot` bump is needed for Phase-8 voice wiring; if we do need the bump, land it as a clean change (likely `flake.lock` only).

Acceptance:
- `~/code/infra` has either (a) no stash, or (b) stash entries are understood + documented.
- Working tree clean or changes are intentional for audio-calls E2E.

## Step 1 — Server readiness check (OpenClaw + sidecar + secret)

Actions:
- `ssh streambot 'systemctl status openclaw-gateway --no-pager -n 50'`
- `ssh streambot 'journalctl -u openclaw-gateway -n 200 --no-pager'`
- Verify secret file: `ssh streambot 'ls -la /run/secrets/openclaw/openai_api_key'`
- Verify env template is present (redacted): `ssh streambot 'sed -E \"s/(OPENAI_API_KEY|ANTHROPIC_API_KEY)=.*/\\1=REDACTED/\" /run/secrets/rendered/openclaw-env'`
- Optional: verify MOQ relay reachability from server (QUIC/UDP is hard to “ping”; just confirm no obvious connect errors in logs during calls).

Acceptance:
- `openclaw-gateway` is active (running).
- No recurring crash loop.
- No log lines indicating STT/TTS misconfiguration (e.g. “OPENAI_API_KEY missing”) when a call is active.

## Step 2 — Verify deployed bot has Phase-8 voice wiring (required for “talking bot”)

Why: the server must have both:
- Updated `marmotd` that can subscribe/publish over real MOQ with MLS frame crypto + opaque labels.
- Updated OpenClaw marmot extension + sidecar protocol that can handle `call_transcript_final -> agent -> send_audio_response`.

Actions (streambot):
- Confirm OpenClaw config validity (should be silent/no “Invalid config”):
  - `ssh streambot 'journalctl -u openclaw-gateway -n 200 --no-pager | tail -n 200'`
- Confirm plugin entry has a config with relays (schema requires it):
  - `ssh streambot \"jq -c '.plugins.entries.marmot' /home/openclaw/.openclaw/openclaw.json\"`
  - Acceptance target: `.plugins.entries.marmot.config.relays` exists and matches channel relays.
- Confirm extension contains the voice wiring markers:
  - `ssh streambot 'grep -nE \"activeCalls|sendAudioResponse|send_audio_response\" /home/openclaw/.openclaw/extensions/marmot/src/channel.ts || true'`
  - `ssh streambot 'grep -nE \"send_audio_response\" /home/openclaw/.openclaw/extensions/marmot/src/sidecar.ts || true'`
  - Acceptance target: `channel.ts` has `activeCalls` and calls a sidecar `sendAudioResponse`; `sidecar.ts` has a `send_audio_response` command.

Acceptance:
- No `openclaw-gateway` “Invalid config” errors for `plugins.entries.marmot`.
- Extension markers present (as above).

## Step 3 — Synthetic transport smoke (laptop → deployed bot, real MOQ relay)

Purpose: prove signaling + real MOQ media transport + crypto/opaque labels end-to-end, without requiring mic/audio devices.

Actions (laptop, in this worktree):
- Ensure you’re using a whitelisted identity:
  - `export PIKA_TEST_NSEC=...` (from gitignored `.env` or environment)
- Run:
  - `cd ~/code/pika/worktrees/audio`
  - `cargo run -p pika_core --bin interop_openclaw_voice -- npub1rtrxx9eyvag0ap3v73c4dvsqq5d2yxwe5d72qxrfpwe5svr96wuqed4p38`
- In parallel, tail server logs:
  - `ssh streambot 'journalctl -u openclaw-gateway -f'`

Acceptance:
- Client prints `ok: interop openclaw voice PASS`.
- Call reaches `CallStatus::Active`.
- Client debug shows media flowing (at least one of `tx_frames`/`rx_frames` increases; `rx_frames > 10` if bot is publishing response audio).
- Server logs show call invite accepted + session started.

## Step 4 — Real voice E2E (device mic → STT → LLM → TTS → audio back)

Purpose: validate the actual product: spoken words go in, bot speaks back.

Actions (iOS preferred, Android ok):
- Configure client to use real MOQ relay:
  - `PIKA_CALL_MOQ_URL=https://moq.justinmoon.com/anon` (if needed)
  - broadcast prefix `pika/calls` (default)
- Log in as a whitelisted identity (Justin real or test key).
- Start a 1:1 chat with the bot and initiate a call.
- Speak a short deterministic phrase (e.g. “what time is it”).
- Tail server logs during the call:
  - `ssh streambot 'journalctl -u openclaw-gateway -f'`

Acceptance:
- Bot produces at least one `call_transcript_final` with non-empty text matching the spoken phrase (allow minor transcription variance).
- Bot produces at least one TTS publish event (or no TTS error logs) shortly after transcript.
- Caller hears an audio response from the bot within ~10 seconds after finishing the phrase.
- Call can be ended cleanly; no service crash.

## Troubleshooting (common failures)

- CreateChat never opens: bot key package not found (kp relay issue). Try overriding kp relays via `PIKA_KEY_PACKAGE_RELAY_URLS=...`.
- Call becomes Active but no transcript: mic audio not flowing (device permissions/session), or bot not in Phase-8 wiring state (re-check Step 2), or STT config/limits.
- Transcript exists but no audio response: TTS failing or `send_audio_response` not wired (re-check Step 2 + logs for TTS errors).

## Step 5 — Make it repeatable (manual lane + deterministic lane)

Actions:
- Manual lane (real OpenAI): document one “golden” manual smoke sequence (commands + expected log strings) and keep it in-repo.
- Deterministic lane (no OpenAI): use fixture envs on the server:
  - `MARMOT_STT_FIXTURE_TEXT=...`
  - `MARMOT_TTS_FIXTURE=1`
  - (Run a call and confirm transcript + audio response happen without external APIs.)

Acceptance:
- There is a documented “manual voice smoke” procedure that someone can run end-to-end in <10 minutes.
- There is a deterministic mode that validates wiring even when OpenAI is unavailable.

## Step 6 — Post-smoke cleanup

Actions:
- Rotate OpenAI key used for testing (OpenAI dashboard) and update `secrets/openclaw.yaml` accordingly.
- Confirm old key no longer works (optional) and new one is deployed.

Acceptance:
- No long-lived test key remains in use.
- Streambot continues to pass Step 1 readiness checks after rotation.
