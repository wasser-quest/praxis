# Transactional host file edits

`inspect_file` and `apply_patch` extend execution contracts with bounded file
edits and compensation. They require an active task with a pinned workflow that
defines checks. Interrupted transactions use a [durable journal](PATCH_RECOVERY.md).
This version requires a Unix host and a writable parent for the workspace. Both use the pinned `ROOT_DIR` and operate on the **host**, even
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
   check name; snapshot originals in memory and persist a private journal outside
   the root. Replace and synchronize the shared workspace revision before staging
   any target. A preflight error changes no target files or shared revision.
2. Recheck each original just before publication. Persist its intent, stage its
   replacement in the same directory, then publish each file with rename
   (or remove it for deletion); creations refuse to clobber an existing file.
   Preserve the permissions of existing files. New files use owner-only `0600`
   permissions.
3. Checks see no runtime staging files after successful publication. Originals
   remain in memory and the durable journal for compensation. Run named checks
   in order using the existing bounded verifier.
   Stop on the first failure, timeout or cancellation. Each author timeout still
   applies, and the whole verification sequence has a 300-second deadline. Inspect edited
   files after each passing check and again before committing evidence.
4. Publish passing receipts together only after every requested check succeeds,
   edited files still match the proposed bytes/permissions, and task identity and
   workspace revision are unchanged. Declared resources for every check must still
   match its sample, including earlier checks after later checks run. A committed
   patch can then satisfy guards after its commit decision is synchronized and
   journal cleanup completes.
5. Otherwise revoke evidence and restore applied files in reverse order. Restore
   only when a file still matches this patch's published bytes and permissions.
   Rebuild restoration files from the in-memory originals when needed; an I/O
   failure during compensation is a rollback conflict.
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
- Transactional edits, inspections, named checks and recovery are serialized
  across tasks and cooperative Praxis processes. Another process fails with a
  busy-workspace error rather than waiting. Raw writes, shell commands, plugins
  and external writers do not participate in the advisory lock.
- Publication is atomic **per file**. Readers can observe a partially published
  multi-file batch; this is a compensating transaction, not an atomic filesystem
  snapshot. Path/hash rechecks catch detectable changes, but there is still a
  check-to-rename race with uncooperative writers or hostile path replacement.
- Only listed files are compensated. Check side effects, build artifacts,
  ownership, timestamps, extended attributes and unrelated files are outside the
  snapshot. Check programs must be trusted and should leave edited sources alone.
- A write-ahead journal and file/directory synchronization support recovery
  after process termination. Recovery runs before the next scoped operation or
  with `praxis recover-patches`; passing receipts are never restored. Filesystem
  durability, conflicts and remaining crash boundaries are documented in
  [patch recovery](PATCH_RECOVERY.md).
- Committed receipts bind the per-task observed-tool revision, the shared durable
  workspace revision, and any author-declared resource hashes. A later cooperating
  patch or recovery invalidates prior receipts across processes, even after a
  rollback and even without resource scopes. Normal commit cleanup preserves the
  new receipts' revision. Guards also recheck resource hashes, detecting external
  differences within declared scopes. These are sampling boundaries, not a frozen
  filesystem; see [execution contracts](EXECUTION_CONTRACTS.md).

Run `cargo test --locked --lib tools::patch_tests` for offline process/file tests.
The ignored `patch_tools_rollback_then_commit_across_all_three_loops` regression
uses scripted model output and real local checks on all three dispatch paths;
it needs the real POML CLI for the message/agent paths.
