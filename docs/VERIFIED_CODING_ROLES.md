# Verified capabilities in normal coding roles

`standard-verified` is an opt-in version of the standard role workflow for an
operator-selected Rust project. It retains normal role selection through
`sm_data.role` and an existing configured Decision-router profile. Coding tasks
use the usual `code`, `expert_programmer`, `debugger`, `senior_dev`,
`code_architect` and `review` states; graph navigation is not needed.

Prepare the project and install `verified_rust` using the commands in the
[reproducible Snake guide](SNAKE_IR_TEST.md). Restart Praxis with an explicit
`WORKSPACE_DIR` or `--workspace-dir`. In the current chat, select
`settings.sm_file = "standard-verified"` and top-level `sm_file` with the same
value. Clear a previously selected manual system template, choose state `code`
and `sm_data.role = "code"`, then begin a new task. The workflow will select
the normal role template. Set this workflow in the default context if newly
created chats should inherit it; already-created chats keep their own context.

| Role | Capabilities / IR |
| --- | --- |
| code, expert_programmer, debugger, senior_dev | inspect R, modify M, build B, test T, complete C |
| code_architect, review | inspect R, build B, test T, complete C; no source modification |
| routing, standard, teach, research | normal conversation, role selection and their declared tools |

Bare capability calls and IR use the same contracts, selected workspace,
receipts and guards. The coding roles offer semantic actions in place of direct
terminal/file writes. Other enabled plugins remain operator-trusted; this is
not a sandbox for arbitrary plugin code. The original `standard` workflow is
still available for existing projects. Non-Rust projects need matching
operator-authored capabilities and checks rather than the Rust example.

## Completion after an edit

The profile declares:

```ini
[action_guards]
_complete = [verified_rust/modify_source, verified_rust/build_workspace, verified_rust/run_workspace_tests]
[action_guard_triggers]
_complete = [verified_rust/modify_source]
```

Without a trigger entry, an action guard retains its existing unconditional
behavior. With an entry, attempting **any** listed capability activates the
guard for the rest of that task. The runtime records attempts independently
of returned output and saved context. Failure, rollback, a role change or
receipt invalidation cannot turn the guard off. A new task has a new ledger.

This lets read-only planning and conversation finish without a fabricated
source edit. Once source editing starts, a verified modification and fresh
passing build/test receipts are required. Entering review or saying “done” does
not bypass them. Named-check `[guards]` remain unconditional even if an action
guard on the same destination has triggers.

Use R to inspect a current source/hash; M supplies the existing relative path,
actual `expected_sha256` and full formatted replacement content. M checks
formatting and compilation and restores the file on failure. B/T use
`{"scope":"workspace"}`. Obtain both after the final edit. Completion text
can accompany C in agent mode.

The profile uses 40 model turns and 80 tool calls in coding roles and the
existing provider output-token policy. These task budgets do not increase the
provider's output limit. Use the documented [long-horizon profile](LLM_RESILIENCE.md)
when large source replacements need an explicitly larger output budget.

## Setup failures

Praxis checks effective opcode tables across declared states and required
capability guards before requesting a model. Missing/disabled owners, absent
contracts, tool-name conflicts, disabled tools and IR targets absent from their
state's trusted allow-list produce an operator setup error. An effective local
table replaces the global one; globally declared mappings that are overridden
in every state are not required.

The error reports workflow, state, target, a code and the operator action.
Tool-time setup errors also include `retryable:false`, `executed:false` and
`verified:false`. Tool enable flags are checked again on subsequent requests
and graph navigation. Fix configuration and start a new task. Do not try an
`owner` operand, different JSON keys or another project root to repair setup.

The preflight does not run verifiers, install packages or prove that cargo,
rustfmt, a network service or project tests will succeed. Those facts require
the actual action receipts. Existing workspace/template validation still
applies before execution.
