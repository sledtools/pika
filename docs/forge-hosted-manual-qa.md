---
summary: Short deploy and manual QA checklist for hosted forge testing on git.pikachat.org
read_when:
  - deploying pika-news in forge mode
  - doing manual QA of branch, inbox, CI, merge, mirror, or nightly behavior
---

# Hosted Forge Manual QA

Use this after deploys or config changes. The goal is to verify the current forge slice without
guessing which surfaces matter.

## Before Starting

- Confirm `/news/admin` loads and `Forge Health` shows `Poller`, `Generation Worker`, and `CI Runner` as `idle` or `active`, not `error`.
- Confirm `Mirror Background` matches the intended mode for the host:
  - `disabled` if `forge_repo.mirror_poll_interval_secs = 0`
  - `idle` or `active` if background mirroring is enabled
- Confirm the admin page shows no unexpected startup issues for:
  - webhook secret
  - canonical repo path
  - mirror remote
  - mirror auth/token

## Branch Push

1. Push a non-`master` branch to the canonical forge remote.
2. Open `/news`.
3. Confirm the branch appears near the top of the open branch feed.
4. Open the branch page and confirm:
   - the summary/detail page renders
   - the branch id is stable and numeric
   - the CI section appears

## CI Visibility

1. Wait for the branch CI lanes to start.
2. Confirm the branch page shows lane-level statuses.
3. Expand at least one lane and confirm logs are visible.
4. If you manually rerun a lane, confirm the rerun shows:
   - `manual rerun of run #...`
   - `manual rerun of lane #...`

## Inbox And Review

1. Sign in as a trusted tester with inbox access.
2. Open `/news/inbox`.
3. Confirm the new branch appears as a branch review item.
4. Open `/news/inbox/review/:id` from the inbox.
5. Confirm:
   - the review page resolves
   - prev/next navigation works
   - dismiss works

## Merge And Durable History

1. Merge the open branch from the branch page as a trusted contributor.
2. Confirm the source branch ref is deleted from the canonical repo.
3. Refresh `/news`.
4. Confirm the branch moves into history.
5. Reopen the merged branch page and confirm it still shows:
   - summary/tutorial content
   - CI history
   - merge commit

## Mirror Sync

1. Open `/news/admin`.
2. Confirm the mirror section clearly shows whether background sync is enabled or disabled.
3. Trigger `Sync Mirror Now`.
4. Confirm the admin page updates:
   - last attempt
   - last success or last failure
   - lagging ref count
5. If mirror auth is intentionally missing, confirm the admin issue text is actionable.

## Nightly Visibility

1. Wait for or manually seed a nightly run on the forge side.
2. Confirm the nightly appears on `/news`.
3. Open the nightly page and confirm lane-level status and logs render.
4. If you rerun a nightly lane, confirm provenance is visible on the nightly page.
