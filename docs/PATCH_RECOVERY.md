# Interrupted file patch recovery

`apply_patch` now records originals and publication intent before touching targets.
On Unix hosts, the next `inspect_file`, `apply_patch`, or `run_check` for that root
recovers an interrupted transaction before proceeding. Guarded transitions and
completion refuse outstanding journals, including workflows without resource
scopes. Recovery revokes live evidence in the recovering process for tasks pinned
to the same root;
it never reconstructs task ownership or passing check receipts.

For immediate recovery without starting services, unlocking secrets, loading
configuration, or contacting a provider, run:

```sh
praxis recover-patches --directory /absolute/workspace
```

The command prints JSON with `outcome`, `transaction_id`, and relative `conflicts`.
Conflicts or invalid journals produce a nonzero exit status. No file contents are
printed. Outcomes are `clean`, `recovered`, `committed_preserved`, or `conflict`.

## Write-ahead protocol

1. Acquire the runtime file-operation mutex and a nonblocking OS lock for the
   canonical workspace. A second Praxis process refuses to inspect, check, edit,
   or recover that root while its lock is held.
2. Validate the complete edit batch in memory. Save versioned originals, modes,
   expected replacement hashes, and an initially empty publication prefix.
   Synchronize the journal file and its directory before creating stages.
3. Before each file's publication, durably advance its intent in the journal.
   Stage replacement bytes under a name derived from the transaction UUID,
   synchronize the data, rename/remove the target, and synchronize its directory.
4. Run all trusted postcondition checks and freshness checks. Persist the commit
   decision while the evidence ledger remains locked, before any passing receipt
   can authorize a guard. Then remove the journal and synchronize its directory.
5. On failure, durably select compensation before restoring targets. Restore in
   reverse intent order only when current contents/existence and permissions
   match the proposed result. Targets already matching the original are accepted
   as unmodified or already restored. Preserve other states as conflicts.

Recovery is repeatable after partial restoration. It also removes this journal's
owned staging files, including partial stages left before publication. Files
outside the intended prefix are not restored. A durable commit decision preserves
targets and only cleans up runtime records; a later external edit is not reverted.

## Storage and operator resolution

Journals live beside the workspace in
`.praxis-patch-journal-<SHA256-of-canonical-root>/pending.json`. The permanent
`lock` file remains after cleanup and must not be replaced while Praxis is active.
Keeping records outside the root prevents journals from changing resource hashes,
including `resources:["."]`. The root's parent must be writable; on Unix, named
checks and inspections also need this directory because they share its lock. The filesystem
root itself cannot be used as a durable patch workspace.

The journal directory is owner-only (`0700`), and records/lock files are owner-only
(`0600`). Originals are stored as plaintext base64: protection is filesystem
permissions, not encryption. Each journal is capped at 8 MiB and retains the
existing 16-file, 1-MiB-per-file, 4-MiB-batch bounds. Version, root, identity, paths,
hashes and modes are validated before recovery. Symlinks, hardlinks, unsafe
ownership/permissions, oversized records and malformed data fail closed. Adjacent
`.praxis-patch-<UUID>-<index>-stage`/`restore` names belong to the runtime.

A conflicted journal stays in place and blocks subsequent guarded work. Preserve
later edits with version control or a separate copy, then resolve each target to
its recorded original or proposed state and rerun recovery. Invalid journals need
operator inspection; there is no model-controlled force, override, or discard tool.
Transaction responses include `journal_id` and `recovery_pending`. A commit cleanup
failure may retain a committed journal: its edits remain committed, but guards
stay blocked until cleanup succeeds. A commit-write failure revokes evidence and
attempts compensation; an I/O obstruction can leave a recovery conflict.

## Guarantee boundaries

Durable patches and recovery require Unix file locks and directory synchronization.
They fail closed on other platforms; existing named checks and inspections remain
available there when no journal is outstanding. The recovery tests use a real subprocess killed
with SIGKILL during its postcondition check, plus intent, conflict, commit and
malformed-record regressions. They do not simulate power loss or certify storage
hardware, network filesystems, or operating-system crash behavior. Durability
depends on the filesystem honoring file and directory synchronization.

Publication remains atomic per file, not per batch. Raw tools, plugins, external
writers, and surviving check processes do not honor this advisory lock. A runtime
crash can leave a check process alive; its side effects remain outside file
compensation. Compare-before-restore detects visible differences, but does not
prevent path races or transient writes restored between samples. An external
write matching the proposed bytes and mode cannot be distinguished from this
transaction's publication. Ownership,
timestamps, extended attributes, build artifacts and check side effects are not
restored. Restarting Praxis creates fresh tasks; rerun checks for new evidence.

Run `cargo test --locked --lib journal_` for recovery regressions. The ignored
`journal_crash_worker` is invoked by the parent crash test and deliberately killed;
it is not a standalone application test.
