---
name: land
description: Land a branch on the canonical forge using git.pikachat.org and ph. Use when the user asks to upstream, land, merge, submit, or finish a branch; when an agent should push a branch, wait for forge CI, inspect failures, and merge it; or when the user wants the old GitHub-PR landing workflow replaced with the forge-native one.
---

# Land

Use this skill when the task is to land code on the forge.

This repo is forge-native now:

- canonical Git is on `git.pikachat.org`
- GitHub is a mirror
- `ph` is the agent-facing control-plane client

The default path is:

1. rebase onto canonical `master`
2. push branch to the forge
3. wait for forge CI with `ph`
4. inspect failures with `ph logs`
5. merge with `ph merge`

Do not default to GitHub PRs.

## Remote policy

Do not silently rename or rewrite a user's remotes.

Before landing, inspect remotes with:

```bash
git remote -v
```

Preferred layout:

- `origin` -> `git@git.pikachat.org:pika.git`
- `github` -> `git@github.com:sledtools/pika.git`

If the repo is not set up that way:

- tell the user
- recommend using `origin` for the forge remote
- only change remotes if the user explicitly asked

## Default landing flow

Assume the current worktree should be used unless the user explicitly wants isolation or continued
parallel work.

Typical flow:

```bash
git fetch origin
git rebase origin/master
git push -u origin HEAD
ph status
ph wait
```

If CI fails:

```bash
ph logs
```

Fix the issue, push again, and repeat.

If CI is green:

```bash
ph merge
```

Then verify:

```bash
git fetch origin
git rev-parse origin/master
git ls-remote github master
```

The GitHub mirror check is useful, but do not block on it unless the user explicitly asks or the
task is about mirror validation.

## When to use a sibling worktree

Only create a sibling worktree when it materially helps:

- the user wants the current worktree free to keep moving
- the branch needs careful cherry-pick or rebase cleanup
- you expect multiple CI fixup pushes and want isolation

If you do create a sibling worktree:

- make a short random directory in `../`
- branch from `origin/master` or cherry-pick the local commits missing from `origin/master`
- keep the current worktree untouched

## `ph` command guidance

Useful commands:

```bash
ph status [branch|id]
ph wait [branch|id]
ph logs [branch|id]
ph merge [branch|id]
ph close [branch|id]
ph url [branch|id]
```

If no branch or id is passed, `ph` may infer the current branch. Prefer explicit branch or id when
the current worktree state is ambiguous.

## Failure handling

Ignore CodeRabbit and Devin unless they point to a concrete independently verified problem.

Focus on:

- forge CI status
- required lane failures
- `ph logs`
- real regressions

If CI appears wedged or the failure is operational rather than code-related, report that clearly
instead of pretending the landing path is healthy.

## Final handoff

When finishing a landing task, include:

- whether you landed from the current worktree or a sibling worktree
- which remote you used for the canonical push
- whether CI passed cleanly or needed fixes
- whether the branch was merged through `ph`
- whether the GitHub mirror caught up if you checked it
