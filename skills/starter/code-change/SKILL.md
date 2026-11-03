---
name: code-change
description: Change code in the workspace and show that the change works. Use for a bug fix, a small feature or a refactor, when the workspace holds the code and a shell is there to check it.
---

# Changing code

1. Find where the change goes: `search_files` for the names in the task,
   `find_files` for the files, then `read_file` with `offset` and `limit`
   for the parts that matter. Read the tests that cover them.
2. Before changing anything, run `run_tests` (narrowed with `filter`) and
   `diagnostics` to see how things stand, so that what fails afterwards is
   known to be yours.
3. Make the smallest change that does it: `edit_file` for one place,
   `apply_patch` for several. Keep the code's own style and names.
4. Add or change a test that fails without the change and passes with it.
5. Run `diagnostics` and `run_tests` again. Read what fails; fix it; run
   them again. Stop after three rounds that do not help, and say what you
   found.
6. Report what you changed, file by file, why, and what the tests say.
   Do not say a change works unless a test you ran shows it.
