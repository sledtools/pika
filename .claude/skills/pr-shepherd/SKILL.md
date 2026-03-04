---
name: pr-shepherd
description: Shepherd a PR through CI and review feedback until it's green and approved
allowed-tools: Bash, Read, Grep, Glob, Edit, Write, Agent, WebFetch, Skill
---

# PR Shepherd

Shepherd a PR through CI checks and review feedback. Monitors CI, fixes build failures, addresses reviewer comments, and repeats until the PR is green with no unaddressed feedback.

Takes an optional PR number argument; defaults to the current branch's PR.

## Overview

```
Pre-flight (once)  →  Main Loop (repeat)  →  Final Validation  →  Exit Report
```

---

## Pre-flight: Rebase and local build

Before entering the main loop, ensure the branch is up to date and builds cleanly.

1. **Rebase onto master:**
```bash
git fetch origin
git rebase origin/master
```
If there are conflicts, fix them, `git rebase --continue`, commit, and push.

2. **Full local build check (Rust + iOS + Android):**
```bash
nix develop -c just qa
```
This runs `fmt`, `clippy`, `test`, `android-assemble`, and `ios-build-sim` inside the nix shell (required for Android SDK and iOS tooling). All five must pass.

3. **Fix any issues** found in build/test/fmt. Commit and force-push the rebased branch:
```bash
git push --force-with-lease
```

Only proceed to the main loop once the local build is green.

---

## Main Loop

Repeat until done:

1. **Check CI** — poll checks until they finish
2. **Check review comments** — delegate to subagent
3. **Fix everything** — address CI failures and review feedback
4. **Push and repeat** — commit, push, go back to step 1

Exit when: all CI checks pass AND no unaddressed review comments remain.

Escalate to the user when:
- Architectural decisions or breaking changes need human judgment
- Conflicting reviewer feedback that needs a tiebreak
- CI failure you can't reproduce or understand after 2 attempts

---

## Step 1: Get PR context

```bash
PR=${1:-$(gh pr view --json number -q .number)}
gh pr view $PR --json number,title,body,headRefName,url,statusCheckRollup
```

## Step 2: Poll CI checks

Poll until all checks complete (no "pending" or "queued" statuses remain):

```bash
gh pr checks $PR --watch
```

If `--watch` isn't available, poll manually:

```bash
gh pr checks $PR
```

Repeat every 30 seconds until all checks show a final state.

## Step 3: Handle CI failures

If any check failed:

1. Get the failed check's logs:
```bash
# List failed checks
gh pr checks $PR --json name,state,conclusion --jq '.[] | select(.conclusion == "FAILURE" or .conclusion == "failure")'

# Get the run ID and fetch logs
gh run list --branch $(gh pr view $PR --json headRefName -q .headRefName) --status failure --json databaseId,name -q '.[0].databaseId'
gh run view <run_id> --log-failed 2>/dev/null | tail -200
```

2. Classify the failure:
   - **Build error** — compilation failures, missing imports, type mismatches. Read the error, locate the file, fix it.
   - **Test failure** — run the failing test locally to reproduce, then fix.
   - **Lint/format** — run `cargo fmt`, fix clippy warnings.
   - **Flaky/infra** — if the error is clearly infrastructure (timeout, network, OOM), re-run the check:
     ```bash
     gh run rerun <run_id> --failed
     ```

3. After fixing, commit and push (Step 5), then go back to Step 2.

**Limit**: If you've attempted the same CI failure 2 times without progress, escalate to the user.

## Step 4: Handle review comments

**Delegate review handling to a subagent** using the Agent tool so the main context stays clean:

```
Agent(subagent_type="general-purpose", prompt="Run /pr-review $PR — fetch all review comments, prioritize human > Devin > CodeRabbit, address actionable feedback, and reply to addressed comments. For comments you disagree with, reply explaining why and sign it 'claude'. Return a summary of what was fixed, what was skipped, and what needs the user's decision.")
```

This keeps the potentially large comment-fetching and code-reading work in an isolated context. The subagent handles human > Devin > CodeRabbit priority ordering and batches fixes.

## Step 5: Commit and push

```bash
cargo fmt -p pikachat -p pika_core 2>/dev/null || true
git add <specific files>
git commit -m "fix: <description>

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
git push
```

Then go back to Step 1.

---

## Final Validation

If any code changes were made during the loop, re-run the full build before reporting success:

```bash
nix develop -c just qa
```

This re-validates Rust (fmt, clippy, test) + Android (assembleDebug) + iOS (simulator build). If anything fails, fix it, commit, push, and do one more loop iteration.

For visual verification of UI changes, record per-platform E2E test videos using the `/e2e-video` skill and upload them to blossom for the PR description.

---

## Exit Conditions

**Exit successfully when:**
- All CI checks pass AND no unaddressed review comments remain
- Local build/test/fmt all pass
- PR is merged or closed

**Exit with escalation to user when:**
- Issue requires human judgment (architecture decisions, breaking changes)
- Review feedback requires human decision (conflicting opinions, significant design changes)
- Same CI failure persists after 2 fix attempts

**Exit with failure report when:**
- Unable to reproduce issue locally
- External service issues (GitHub API down, CI infrastructure problems)

**On exit, always report:**
- Summary of what was fixed
- Number of CI fix rounds
- Any feedback that was intentionally skipped (with rationale)
- **Link to the PR** (e.g., `https://github.com/sledtools/pika/pull/$PR`) — always include this so the user can find it easily
- **If any items were "tracked for follow-up"**, list them and ask the user if they want GitHub issues opened for them. Use `gh issue create` if they say yes.
