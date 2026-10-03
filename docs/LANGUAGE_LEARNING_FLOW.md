# Interactive language-learning graph

Select `language-learning` as the current chat's workflow and `setup` as its
active state, then send an ordinary chat message. Each state uses
`language-flow`, disables persona Decision routing and selects the chat path
with `max_llm_turns = 1`; tool-only follow-ups remain bounded to eight calls.
No Rust project or `verified_rust` plugin is required.

Try:

> Teach me conversational French at A1 level. Explain in English. My goal is
> ordering food in a café. Teach one small concept and ask one question at a
> time, then wait for my reply. Do not save progress yet.

Or choose another target/explanation language and level. The flow asks for
missing choices and whether progress should be saved. Existing selected
language-specific profiles can be reused.

| Node | Behavior |
| --- | --- |
| setup | Collect missing language, level and goal; ask about saving progress |
| lesson | Teach one small concept with a useful example |
| practice | Ask one exercise and wait for a real user message |
| feedback | Correct that reply gently and explain why; save only agreed progress |
| review | Recap and offer another question or completion |
| done | Complete only when the learner wants to finish; include a visible recap |

Navigation uses the runtime's current edge indices and the active IR table:
N = `agent_next`, K = `agent_back`, and C exists only in `done`. For example,
from practice, edge 0 requests feedback and edge 1 requests review. Back history
belongs to the current task; the persisted current node carries the lesson
across user turns. Do not call C merely to finish asking a question: returning
text waits at the current node; completed sessions restart at setup.

## Verified inbound-message guard

The workflow declares:

```ini
[user_reply_guards]
feedback = [practice]
```

Praxis records the persisted inbound user message ID and the graph node active
when it arrived. Entering feedback requires input received at practice with
no subsequent state transition. Entering practice and then immediately
attempting feedback in the same tool chain is rejected without changing state
or navigation history. A new message, including authenticated injected input,
can satisfy the guard. A model-written `answer_received` flag cannot.

This verifies an input event, not the meaning of the answer or its linguistic
correctness. Ambiguous transcripts should be clarified before feedback. Grade
unassisted and assisted recall differently; never infer pronunciation from
text or award a correct recall for the tutor's own answer.

## Progress and memory

Every stage uses the existing `language_instructor` memory mode. Default
standard/shared memory is not a destination for learning progress. The learner
can choose a language-specific profile with the existing profile tools.
Create/load a profile only when needed and with the learner's consent; teach
without claiming persistence when saving is declined or tools are unavailable.

Use `memory_get` before each update, preserve the full deck and other languages,
and pass `expected_profile` and `expected_value` to `memory_set`. A successful
write confirms persistence; it does not independently verify a grade. Preserve
the existing review/XP ledger to avoid double-awarding the same answer and
reconcile partial card/XP writes without replaying a successful award.

The prompt reuses the existing due-card snapshot, interval rules and gentle
correction instructions. It does not introduce a second SRS implementation or
silently activate paid audio services. The graph and actual prompts/tool results
are visible in Graphs and Messages.

## Reproduction checks

1. Start at setup with the example prompt. Expect a short lesson and one
   question, leaving the graph at practice.
2. Attempting practice → feedback before replying must be blocked by the
   inbound-message guard.
3. Reply to the exercise. Expect feedback using that exact reply.
4. Ask for another question; expect practice and another wait.
5. Ask to finish. Expect a recap through review → done → C.
6. With saving declined, expect no memory writes. With saving agreed, inspect
   confirmed writes in the selected learning profile and unchanged unrelated
   profiles/users.
