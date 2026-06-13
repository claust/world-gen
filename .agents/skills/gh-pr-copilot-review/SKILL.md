---
name: gh-pr-copilot-review
description: Commit local changes, push a branch, create a GitHub pull request, request GitHub Copilot as reviewer, verify Copilot actually started through PR events, and monitor Copilot review progress. Use when the user asks Codex to publish changes to GitHub, open a PR, request Copilot review, check Copilot review progress, or set up periodic PR review monitoring.
---

# GitHub PR Copilot Review

## Overview

Publish local work to GitHub end to end: inspect scope, branch, stage, commit, push, create a PR, request reviewer `Copilot`, verify the request through GitHub issue events, and monitor Copilot review output.

Codex does not receive passive GitHub callbacks in an ordinary thread. For follow-up progress, actively poll GitHub or create a Codex heartbeat/automation when the user asks to keep watching.

## Publish Workflow

When the user invokes this skill, execute the workflow automatically when the intended change scope is clear. Ask before continuing only if the worktree contains unrelated changes, the target branch/base is ambiguous, authentication is missing, or an operation would be destructive.

1. Inspect the worktree before staging:

   ```bash
   git status -sb
   git diff --stat
   git diff
   ```

2. Stage only the intended files. Do not use broad staging when unrelated changes exist.

3. Ensure the branch is suitable:

   - If on a detached HEAD, `main`, `master`, or the default branch, create a `codex/<short-description>` branch.
   - If already on a feature branch, keep it unless the user asks for a different branch.

4. Commit with a concise message after checking the staged diff:

   ```bash
   git diff --cached --stat
   git diff --cached
   git commit -m "<message>"
   ```

5. Run relevant validation for the change when practical. For documentation-only changes, explicitly say no code tests were needed.

6. Push with upstream tracking:

   ```bash
   git push -u origin "$(git branch --show-current)"
   ```

7. Create the PR. Default to the remote default branch unless the user specifies a target:

   ```bash
   gh pr create --base <base> --head "$(git branch --show-current)" --title "<title>" --body "<body>" --reviewer Copilot
   ```

   If PR creation fails only because reviewer `Copilot` could not be requested, create the PR without a reviewer, then request Copilot with `gh pr edit`.

8. Request Copilot exactly, using the reviewer name `Copilot`:

   ```bash
   gh pr edit <pr-number-or-url> --add-reviewer Copilot
   ```

9. Verify Copilot through issue events, not only `reviewRequests`. GitHub may leave `reviewRequests` empty for Copilot even when the request succeeded:

   ```bash
   gh api repos/<owner>/<repo>/issues/<pr-number>/events --paginate
   ```

   Success signals:

   - `event: review_requested` with `requested_reviewer.login: Copilot`
   - `event: copilot_work_started` with `performed_via_github_app.slug: copilot-pull-request-reviewer`

## Monitor Copilot Review

Check progress by reading reviews, review comments, issue comments, status checks, and issue events:

```bash
gh pr view <pr-number-or-url> --json latestReviews,comments,reviewDecision,statusCheckRollup,mergeStateStatus,reviewRequests
gh api repos/<owner>/<repo>/pulls/<pr-number>/reviews
gh api repos/<owner>/<repo>/pulls/<pr-number>/comments
gh api repos/<owner>/<repo>/issues/<pr-number>/events --paginate
```

Treat Copilot as done when it submits a review from `copilot-pull-request-reviewer[bot]` or `Copilot`. Report:

- review state and submitted time
- summary review body
- inline comments with file paths and comment URLs
- CI status
- whether the branch is behind the base branch

## Periodic Follow-Up

If the user asks to watch, monitor, check back, notify them, or get callbacks, create a Codex automation rather than claiming passive callbacks exist.

- Prefer a heartbeat automation attached to the current thread for short-term review monitoring.
- Use a cron automation for detached repository monitoring.
- The automation prompt should ask Codex to inspect the PR using the GitHub CLI/API commands above and report only meaningful changes: new Copilot review, new comments, failed checks, or merge readiness.

## Important Details

- Use reviewer `Copilot`, capitalized exactly. The app slug observed in PR events is `copilot-pull-request-reviewer`.
- `gh pr edit --add-reviewer copilot` can return success without a visible `reviewRequests` entry. Always verify through issue events.
- A submitted Copilot review may appear as `copilot-pull-request-reviewer[bot]` in PR reviews and as `Copilot` in review comments.
- Keep PRs draft only when the user asks for draft or the local workflow requires it. Otherwise follow the user's requested PR state.
- Include GitHub Copilot review status in the final handoff after creating the PR.
