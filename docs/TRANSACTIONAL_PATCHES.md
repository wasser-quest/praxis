# Transactional host file edits

`inspect_file` and `apply_patch` extend execution contracts with bounded file
edits and compensation. They require an active task with a pinned workflow that
defines checks. Both use the pinned `ROOT_DIR` and operate on the **host**, even
when terminal tools use a VM. The bundled `verified-coding` workflow allows these
tools instead of raw writes and shell commands in its coding group. Other
workflows can activate or discover them explicitly.

First call `inspect_file({"path":"src/example.rs"})`. It returns the full UTF-8
content, existence, and a lowercase SHA-256 hash. A nonexistent file returns
`exists:false`, `sha256:null`, and `content:null`. Copy the returned hash into an
edit; do not invent it or derive it from a shortened tool-output view.

```json
{
  "edits": [
    {
      "path": "src/example.rs",
      "expected_sha256": "<64-character SHA-256 from inspect_file>",
      "content": "full replacement text\n"
    },
    {
      "path": "src/new.rs",
      "expected_sha256": null,
      "content": "new file text\n"
    }
  ],
  "checks": ["build", "tests"]
}
```

`apply_patch` accepts a JSON list of complete replacements, creations and
deletions; it does not accept unified diff text. All three edit fields are
required. `expected_sha256:null` requires that the file does not exist. For
deletion, supply the existing hash and `content:null`. Null content cannot delete
an already nonexistent file. No model-supplied command, cwd, timeout, receipt,
permission change, or additional field is accepted. Named checks come from the
author's pinned `[checks]` policy. Selecting only some checks does not satisfy
guards that require additional ones.

## Execution and evidence

1. Revoke previous evidence. Validate every path, expected hash, data limit and
   check name; snapshot originals and stage replacement/restoration files in the
   same directories. A preflight error changes no target files.
2. Recheck each original just before publication. Publish each file with rename
   (or remove it for deletion); creations refuse to clobber an existing file.
   Preserve the permissions of existing files. New files use the temporary-file
   default permissions, normally owner-only access on Unix.
3. Run named checks in order using the existing bounded verifier. Stop on the
   first failure, timeout or cancellation. Each author timeout still applies, and
   the whole verification sequence has a 300-second deadline. Inspect edited
   files after each passing check and again before committing evidence.
4. Publish passing receipts together only after every requested check succeeds,
   edited files still match the proposed bytes/permissions, and task identity and
   workspace revision are unchanged. A committed patch can then satisfy guards.
5. Otherwise revoke evidence and restore applied files in reverse order. Restore
   only when a file still matches this patch's published bytes and permissions.
   Preserve changed files and report conflicts instead of overwriting them.

Normal tool-output archival retains the transaction receipt, proposed before and
after hashes, check outcomes and bounded output. The transaction outcome is:

| Outcome | Meaning |
| --- | --- |
| `committed` | Every requested check passed; fresh evidence was published. |
| `rolled_back` | The transaction failed and all applied edits were restored. |
| `rollback_conflict` | At least one file could not safely be restored; inspect the listed paths and report incomplete work. |

No check in a rolled-back or conflicted transaction is marked `verified`, even
if an earlier check exited zero. Timeouts and task cancellation restore owned
edits before returning the result. Dropping/aborting an in-process future also
attempts restoration through a scope guard; a dropped future cannot return an
archived receipt, and restoration conflicts are logged only as a count.

## Bounds and practical limits

- 1–16 unique normalized relative paths; no absolute paths, `.`/`..` components,
  backslashes, symlinks, or nonregular files. Parent directories must exist.
  Hardlinks are rejected on Unix. Each original or replacement is at most 1 MiB;
  originals plus replacements total at most 4 MiB.
- 1–8 unique named checks. Check stdout and stderr in the transaction response
  are each capped at 64 KiB, with truncation flags; full edited contents are not
  included in the receipt.
- This runtime serializes transactional edits, inspections and named checks
  across tasks. Raw writes, shell commands, plugins, other runtime processes and
  external writers do not participate in that lock.
- Publication is atomic **per file**. Readers can observe a partially published
  multi-file batch; this is a compensating transaction, not an atomic filesystem
  snapshot. Path/hash rechecks catch detectable changes, but there is still a
  check-to-rename race with uncooperative writers or hostile path replacement.
- Only listed files are compensated. Check side effects, build artifacts,
  ownership, timestamps, extended attributes and unrelated files are outside the
  snapshot. Check programs must be trusted and should leave edited sources alone.
- In-memory snapshots and scope guards do not recover from process termination,
  power loss, or a host crash. There is no durable recovery journal or directory
  fsync protocol. Use an isolated workspace and version control for that boundary.
- Committed receipts still use the existing per-task observed-tool revision;
  subsequent edits by another task or external writer are not detected by a
  workspace tree hash in this slice.

Run `cargo test --locked --lib tools::patch_tests` for offline process/file tests.
The ignored `patch_tools_rollback_then_commit_across_all_three_loops` regression
uses scripted model output and real local checks on all three dispatch paths;
it needs the real POML CLI for the message/agent paths.
