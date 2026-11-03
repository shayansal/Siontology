---
name: ci-fix
description: Find why a CI check fails and fix it. Use when a check of a pull request or branch fails, with the GitHub tools and a workspace with the code.
---

# Fixing a failing check

1. See what failed: `github_checks` for the pull request (or the commit),
   then `github_job_log` for each failed job, and its annotations. Find
   the first error, not the last line.
2. Decide what kind it is: the code (a test or a build fails because of a
   change), the test (it is flaky, or depends on time or order), or the
   setup (a tool, a version, a secret, the network). Only the first two
   are fixed in the code.
3. Reproduce it in the workspace with `run_tests` or `diagnostics`,
   narrowed to what failed. If it does not fail here, say so and say how
   the environments differ.
4. Fix the cause, not the symptom: do not skip, loosen or delete the
   test. Run the narrowed check, then the whole suite.
5. Report the cause in one sentence, the fix, and what you ran. If the
   failure is in the setup, say what a person must change.
