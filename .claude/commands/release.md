Create a release PR from main to production.

Steps:
1. Fetch latest from `origin/main` and update the local `main` branch
2. Compare `origin/production..origin/main` to identify all commits and changed files since the last release
3. Group commits by feature/PR for the release summary
4. Create a PR from `main` to `production` with:
   - Summary listing each feature with its PR reference

