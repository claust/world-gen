Create a release PR from dev to master.

Steps:
1. Fetch latest from `origin/dev` and update the local `dev` branch
2. Compare `origin/master..origin/dev` to identify all commits and changed files since the last release
3. Group commits by feature/PR for the release summary
4. Create a PR from `dev` to `master` with:
   - Summary listing each feature with its PR reference
   - Stats (files changed, lines, commit count)
   - Test plan checklist
