#!/usr/bin/env python3
"""20 natural tasks, paired real Praxis runs; state selection != answer quality.

Needs a compiled library test executable, real POML and an already running model.
Never starts a GPU instance. Rubrics stay in the observer, outside model context.
"""
import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import statistics
import subprocess
import time
import urllib.request
import urllib.parse


def load(path, default=None):
    return json.loads(path.read_text()) if path.exists() else default


def receipt_text(receipt):
    text = receipt.get('content', '')
    if not isinstance(text, str):
        return ''
    try:
        value = json.loads(text)
        if isinstance(value, dict) and isinstance(value.get('text'), str):
            return value['text']
    except (ValueError, TypeError):
        pass
    payload, marker, footer = text.rpartition('\n\n[Saved tool response] output_id=')
    archived = re.fullmatch(
        r'[^\n]+ \n(\d+) source characters; (\d+) / (\d+) bytes retained; storage_truncated=false\. '
        r'Use read_tool_result to inspect sections without re-execution\. Retained up to 7 days, '
        r'subject to quotas and actual history/session deletion\.', footer)
    if marker and archived:
        characters, retained, source = map(int, archived.groups())
        if characters == len(payload) and retained == source == len(payload.encode('utf-8')):
            return payload
    return text


def analyse_run(root):
    root = Path(root)
    fixture = load(root / 'fixture.json')
    task, arm = fixture['task'], fixture['arm']
    result = load(root / 'result.json', {})
    http = result.get('http_result') or {}
    final_state = (result.get('final_context') or {}).get('active_state')
    receipts = {}
    for message in result.get('history', []):
        if message.get('role') == 'tool':
            receipts.setdefault(message.get('tool_call_id'), []).append(message)
    state = task['initial_state']
    transitions, deterministic, no_ops, observed, attempts = [], [], [], [], []
    decision_transitions, state_attempts = [], []
    trace = load(root / 'decision-routes.json', {})
    routes = trace.get('events', [])
    trace_complete = trace.get('complete') is True if arm == 'decision' or trace else True
    route_statuses = Counter()
    decision_elapsed_ms = 0
    read_seen = False
    appropriate_after_evidence = False
    prompt_tokens = completion_tokens = 0
    calls = sorted(root.glob('call-*.request.json'))
    usage_complete = bool(calls)
    pending_routes = list(routes)

    def apply_routes(before_call, actual):
        nonlocal state, trace_complete, decision_elapsed_ms
        matching = [event for event in pending_routes if event.get('before_call') == before_call]
        reevaluate = (fixture.get('decision_profile') or {}).get('reevaluate', 'every_step')
        if arm == 'decision' and reevaluate == 'every_step' and before_call < len(calls) and len(matching) != 1:
            trace_complete = False
        for event in matching:
            pending_routes.remove(event)
            route = event.get('route') or {}
            status = route.get('status')
            if route.get('source') != 'decision' or status not in {'applied', 'unchanged', 'low_probability', 'failed', 'stale_context'}:
                trace_complete = False
                continue
            route_statuses[status] += 1
            elapsed = route.get('elapsed_ms')
            if isinstance(elapsed, (int, float)) and elapsed >= 0:
                decision_elapsed_ms += elapsed
            else:
                trace_complete = False
            if status == 'applied':
                target = route.get('to_state')
                if route.get('from_state') != state or not isinstance(target, str) or target == state:
                    trace_complete = False
                    continue
                decision_transitions.append(dict(from_state=state, to_state=target,
                                                 before_call=before_call, after_evidence_read=read_seen))
                state = target
        if matching and actual != state:
            trace_complete = False

    for request_path in calls:
        number = int(request_path.name.split('.')[0].split('-')[1])
        request = load(request_path)
        actual = (request.get('observed_state') or {}).get('active_state')
        apply_routes(number, actual)
        if actual is not None:
            observed.append(actual)
            if actual != state:
                deterministic.append({'from': state, 'to': actual, 'before_call': number})
                state = actual
            if read_seen and actual in task['acceptable_states']:
                appropriate_after_evidence = True
        response = (load(root / f'call-{number:02}.response.json', {}).get('response') or {})
        usage = response.get('usage') or {}
        valid_usage = all(type(usage.get(key)) is int and usage[key] >= 0
                          for key in ['prompt_tokens', 'completion_tokens'])
        usage_complete = usage_complete and valid_usage
        if valid_usage:
            prompt_tokens += usage['prompt_tokens']
            completion_tokens += usage['completion_tokens']
        for call in response.get('tool_calls') or []:
            name = call.get('function', {}).get('name', '')
            attempts.append(name)
            try:
                args = json.loads(call['function']['arguments'])
            except (ValueError, KeyError, TypeError):
                args = {}
            if not isinstance(args, dict):
                args = {}
            if name == 'set_context' and args.get('key') in {'active_state', 'settings.active_state'}:
                state_attempts.append({'call': number, 'tool_call_id': call.get('id')})
            matches = receipts.get(call.get('id'), [])
            if len(matches) != 1 or matches[0].get('tool_name') != name:
                continue  # generation/stream fragments are not execution
            text = receipt_text(matches[0])
            # Confirm the known fixture path and bytes in a runtime tool record.
            # A generated read call or prose claiming success is insufficient.
            expected_path = fixture.get('fixture_path')
            if (name == 'read_file' and isinstance(expected_path, str)
                    and args.get('path') == expected_path and text == task.get('fixture')):
                read_seen = True
            if name != 'set_context' or not text.startswith("Context key 'active_state' set"):
                continue
            if args.get('key') != 'active_state' or not isinstance(args.get('value'), str):
                continue
            change = {'from': state, 'to': args['value'], 'call': number,
                      'tool_call_id': call['id'], 'after_evidence_read': read_seen}
            (no_ops if state == args['value'] else transitions).append(change)
            state = args['value']
    apply_routes(len(calls), final_state)
    if pending_routes or (arm == 'decision' and not routes):
        trace_complete = False
    if final_state is not None and state != final_state:
        deterministic.append({'from': state, 'to': final_state, 'after_call': 'last'})
    answer = http.get('response') or ''
    answer = answer if isinstance(answer, str) else ''
    checks = [{'pattern': pattern, 'passed': bool(re.search(pattern, answer, re.I | re.S))}
              for pattern in task['checks']]
    complete = http.get('success') is True
    evidence_ok = 'fixture' not in task or read_seen
    policy = (not state_attempts if arm == 'fixed' else
              len(state_attempts) <= 1 and all(c['call'] == 0 for c in state_attempts)
              if arm == 'entry' else True)
    policy = policy and trace_complete and not deterministic and (not state_attempts if arm == 'decision' else not routes)
    return dict(id=task['id'], title=task.get('title', ''), arm=arm, completed=complete,
                inference_kind=fixture.get('inference_kind', 'unknown'),
                task_rubric_ok=complete and evidence_ok and all(c['passed'] for c in checks),
                checks=checks, evidence_read=read_seen, evidence_requirement_met=evidence_ok,
                appropriate_state_used=any(s in task['acceptable_states'] for s in observed),
                appropriate_state_after_evidence=appropriate_after_evidence,
                final_state=final_state, final_state_matches=final_state in task['acceptable_states'],
                observed_states=observed, model_transitions=transitions,
                deterministic_transitions=deterministic, redundant_state_writes=no_ops,
                decision_transitions=decision_transitions, decision_calls=sum(route_statuses.values()),
                decision_statuses=dict(route_statuses), decision_elapsed_ms=decision_elapsed_ms,
                routing_trace_complete=trace_complete, model_state_attempts=state_attempts,
                policy_followed=policy, tool_attempts=dict(Counter(attempts)),
                blocked_batches=len(list(root.glob('call-*.guard.json'))),
                model_calls=len(calls), usage_complete=usage_complete,
                prompt_tokens=prompt_tokens if usage_complete else None,
                completion_tokens=completion_tokens if usage_complete else None,
                elapsed_s=round(result['elapsed_ms']/1000, 3)
                if type(result.get('elapsed_ms')) in {int, float} and result['elapsed_ms'] >= 0 else None,
                error=http.get('error') or result.get('transport_error'), artifacts=str(root))


def compare_arms(rows, baseline, candidate):
    by_key = {(r['id'], r['arm']): r for r in rows}
    if len(by_key) != len(rows):
        raise ValueError('Duplicate task/arm; repeated runs must not overwrite paired evidence')
    pairs = [(by_key[(i, baseline)], by_key[(i, candidate)])
             for i in sorted({r['id'] for r in rows})
             if (i, baseline) in by_key and (i, candidate) in by_key]
    token_pairs = [(a, b) for a, b in pairs if a.get('usage_complete', True) and b.get('usage_complete', True)]
    latency_pairs = [(a, b) for a, b in pairs if a['elapsed_s'] is not None and b['elapsed_s'] is not None]
    return dict(baseline=baseline, candidate=candidate, paired_tasks=len(pairs),
                rubric_improved_tasks=sum(not a['task_rubric_ok'] and b['task_rubric_ok'] for a, b in pairs),
                rubric_regressed_tasks=sum(a['task_rubric_ok'] and not b['task_rubric_ok'] for a, b in pairs),
                rubric_equal_tasks=sum(a['task_rubric_ok'] == b['task_rubric_ok'] for a, b in pairs),
                compliant_rubric_improved_tasks=sum(not (a['task_rubric_ok'] and a.get('policy_followed', True))
                                                   and b['task_rubric_ok'] and b.get('policy_followed', True) for a, b in pairs),
                compliant_rubric_regressed_tasks=sum(a['task_rubric_ok'] and a.get('policy_followed', True)
                                                    and not (b['task_rubric_ok'] and b.get('policy_followed', True)) for a, b in pairs),
                appropriate_state_delta=sum(int(b['appropriate_state_used'])-int(a['appropriate_state_used']) for a,b in pairs),
                token_paired_tasks=len(token_pairs),
                prompt_token_delta=sum(b['prompt_tokens']-a['prompt_tokens'] for a,b in token_pairs) if token_pairs else None,
                completion_token_delta=sum(b['completion_tokens']-a['completion_tokens'] for a,b in token_pairs) if token_pairs else None,
                latency_paired_tasks=len(latency_pairs),
                elapsed_s_delta=round(sum(b['elapsed_s']-a['elapsed_s'] for a,b in latency_pairs), 3) if latency_pairs else None)


def write_reports(output, rows, requested):
    if len({(r['id'], r['arm']) for r in rows}) != len(rows):
        raise ValueError('Duplicate task/arm in report')
    arms = sorted({r['arm'] for r in rows})
    totals = {}
    for arm in arms:
        selected = [r for r in rows if r['arm'] == arm]
        elapsed = [r['elapsed_s'] for r in selected if r['elapsed_s'] is not None]
        totals[arm] = dict(runs=len(selected), completed=sum(r['completed'] for r in selected),
                          rubric_passed=sum(r['task_rubric_ok'] for r in selected),
                          appropriate_state_used=sum(r['appropriate_state_used'] for r in selected),
                          policy_followed=sum(r['policy_followed'] for r in selected),
                          model_transitions=sum(len(r['model_transitions']) for r in selected),
                          post_evidence_transitions=sum(c['after_evidence_read'] for r in selected for c in r['model_transitions']),
                          deterministic_transitions=sum(len(r['deterministic_transitions']) for r in selected),
                          decision_transitions=sum(len(r['decision_transitions']) for r in selected),
                          decision_calls=sum(r['decision_calls'] for r in selected),
                          decision_elapsed_ms=sum(r['decision_elapsed_ms'] for r in selected),
                          routing_trace_complete=sum(r['routing_trace_complete'] for r in selected),
                          redundant_state_writes=sum(len(r['redundant_state_writes']) for r in selected),
                          usage_reported_runs=sum(r['usage_complete'] for r in selected),
                          prompt_tokens=sum(r['prompt_tokens'] for r in selected) if all(r['usage_complete'] for r in selected) else None,
                          completion_tokens=sum(r['completion_tokens'] for r in selected) if all(r['usage_complete'] for r in selected) else None,
                          latency_reported_runs=len(elapsed),
                          median_elapsed_s=round(statistics.median(elapsed),3) if elapsed else None)
    comparisons = [compare_arms(rows, baseline, candidate)
                   for candidate in ['continuous', 'decision'] for baseline in ['fixed', 'entry', 'continuous']
                   if baseline in arms and candidate in arms and baseline != candidate]
    experiment_status = 'complete' if len(rows) == requested and requested > 0 else 'incomplete'
    summary = dict(requested_runs=requested, completed_runs=len(rows), arms=totals, comparisons=comparisons,
                   experiment_status=experiment_status, decision_conclusion='insufficient_evidence',
                   caveats=['Rubric checks are automated proxies, not proof of full semantic correctness.',
                            'A higher state-switch count is not itself a benefit.',
                            'Runtime initialization/non-model changes are counted separately.',
                            'Latency includes queueing; concurrent GPU load is not controlled.',
                            'Token counts cover the solving model only; classifier token usage is unavailable.',
                            'Router elapsed time includes overhead; end-to-end latency already includes it.',
                            'Twenty distinct tasks with four arms means eighty task runs, not eighty distinct tasks.',
                            'One run per task/arm measures this corpus, not general reliability or statistical significance.',
                            'Scripted tests and partial pilots do not establish live-model behavior.'])
    decision_pairs = next((c for c in comparisons if c['baseline'] == 'fixed' and c['candidate'] == 'decision'), None)
    full_corpus = {(f'{number:02}', arm) for number in range(1,21)
                   for arm in ['fixed', 'entry', 'continuous', 'decision']}
    if (experiment_status == 'complete' and {(r['id'], r['arm']) for r in rows} == full_corpus
            and decision_pairs and decision_pairs['paired_tasks'] == 20
            and totals['fixed']['runs'] == totals['decision']['runs'] == 20
            and all(r['routing_trace_complete'] and r['inference_kind'] == 'live' for r in rows)):
        wins, losses = decision_pairs['compliant_rubric_improved_tasks'], decision_pairs['compliant_rubric_regressed_tasks']
        summary['decision_conclusion'] = ('more_compliant_rubric_passes_on_this_corpus' if wins > losses else
                                          'fewer_compliant_rubric_passes_on_this_corpus' if wins < losses else
                                          'no_net_compliant_rubric_gain_on_this_corpus')
    for name, data in [('rows.json', rows), ('summary.json', summary)]:
        temporary = output / (name + '.tmp')
        temporary.write_text(json.dumps(data, ensure_ascii=False, indent=2))
        temporary.replace(output / name)
    md = ['# 20-task real-state experiment', '',
          f"Status: {experiment_status}. Decision conclusion: {summary['decision_conclusion']}.", '',
          '| Arm | Runs | Completed | Rubric OK | Appropriate state used | Policy OK | Model transitions | Decision transitions | Decision calls |',
          '|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|']
    for arm, s in totals.items():
        md.append(f"| {arm} | {s['runs']} | {s['completed']} | {s['rubric_passed']} | {s['appropriate_state_used']} | {s['policy_followed']} | {s['model_transitions']} | {s['decision_transitions']} | {s['decision_calls']} |")
    md += ['', '## Paired comparisons', '', '```json', json.dumps(comparisons, indent=2), '```', '',
           '## Per-task traces', '', '| Task | Arm | Actual model-request states | Rubric OK |', '|---|---|---|---|']
    for row in rows:
        md.append(f"| {row['id']} | {row['arm']} | {' → '.join(row['observed_states'])} | {row['task_rubric_ok']} |")
    md += ['', '## Limits', *('- '+s for s in summary['caveats']), '']
    (output / 'REPORT.md').write_text('\n'.join(md))
    return summary


def build_parser():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--test-binary', required=True, type=Path)
    parser.add_argument('--runtime-root', type=Path, default=Path.cwd())
    parser.add_argument('--url', required=True)
    parser.add_argument('--model', required=True)
    parser.add_argument('--poml-cli', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--cases', default=','.join(f'{i:02}' for i in range(1,21)))
    parser.add_argument('--arms', default='fixed,entry,continuous,decision')
    parser.add_argument('--decision-profile', type=Path, help='Trusted profile; default: runtime-root/decisions/task-router.json')
    parser.add_argument('--decision-url', help='Override the full /v1/decision endpoint in the isolated snapshot only')
    parser.add_argument('--prepare-only', action='store_true', help='Freeze the experiment without any network requests/inference')
    parser.add_argument('--resume', action='store_true', help='Continue an existing snapshot; completed task/arm pairs are never rerun')
    parser.add_argument('--allow-paid-live-inference', action='store_true')
    return parser


def digest(path):
    sha = hashlib.sha256()
    with path.open('rb') as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b''):
            sha.update(chunk)
    return sha.hexdigest()


def validate_url(url):
    value = urllib.parse.urlsplit(url)
    if (value.scheme not in {'http', 'https'} or not value.hostname or value.username
            or value.password or value.query or value.fragment):
        raise ValueError('Use an HTTP(S) endpoint without embedded credentials, query or fragment')
    return url


def prepare_experiment(args, cases, arms):
    source = args.runtime_root.resolve()
    profile = None
    if 'decision' in arms:
        profile = load((args.decision_profile or source / 'decisions/task-router.json').resolve())
        if not isinstance(profile, dict):
            raise ValueError('A trusted Decision profile is required')
        profile['endpoint'] = validate_url(args.decision_url or profile['endpoint'])
    configuration = dict(cases=cases, arms=arms, url=validate_url(args.url.rstrip('/')), model=args.model,
                         decision_profile=profile, test_binary_sha256=digest(args.test_binary.resolve()),
                         observer_sha256=digest(Path(__file__).resolve()),
                         poml_cli_sha256=digest(args.poml_cli.resolve()))
    output = args.output.resolve()
    if args.resume:
        plan = load(output / 'plan.json')
        if not plan or plan.get('version') != 2 or plan['configuration'] != configuration:
            raise ValueError('Resume configuration differs; use the original inputs or a fresh output directory')
        verify_snapshot(plan)
        return plan
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    frozen = output / 'runtime'
    frozen.mkdir(mode=0o700)
    for name in ['contexts', 'templates']:
        shutil.copytree(source / name, frozen / name)
    (frozen / 'tests/fixtures').mkdir(parents=True)
    shutil.copy2(source / 'tests/fixtures/20-tasks.json', frozen / 'tests/fixtures/20-tasks.json')
    (frozen / 'scripts').mkdir()
    shutil.copy2(Path(__file__).resolve(), frozen / 'scripts/bench_state_machine.py')
    binary = frozen / 'praxis-tests'
    shutil.copy2(args.test_binary.resolve(), binary)
    binary.chmod(0o700)
    if profile is not None:
        (frozen / 'decisions').mkdir()
        (frozen / 'decisions/task-router.json').write_text(json.dumps(profile, ensure_ascii=False, indent=2))
    plan = dict(version=2, configuration=configuration, requested_runs=len(cases)*len(arms),
                runtime_root=str(frozen), test_binary=str(binary), poml_cli=str(args.poml_cli.resolve()),
                decision_profile=profile,
                hashes={str(path.relative_to(frozen)): digest(path) for path in sorted(frozen.rglob('*')) if path.is_file()})
    (output / 'plan.json').write_text(json.dumps(plan, ensure_ascii=False, indent=2))
    return plan


def verify_snapshot(plan):
    root = Path(plan['runtime_root'])
    for name, expected in plan['hashes'].items():
        if digest(root / name) != expected:
            raise ValueError('Frozen experiment runtime changed; refuse to mix results')
    if digest(Path(plan['poml_cli'])) != plan['configuration']['poml_cli_sha256']:
        raise ValueError('POML CLI changed; refuse to mix results')
    if digest(Path(__file__).resolve()) != plan['configuration']['observer_sha256']:
        raise ValueError('Experiment observer changed; refuse to mix results')


def backend_ready(url, model, headers=None):
    def get(path):
        request = urllib.request.Request(url.rstrip('/') + path, headers=headers or {})
        with urllib.request.urlopen(request, timeout=15) as response:
            if response.status != 200:
                raise ValueError('Backend is not ready')
            body = response.read(1024*1024+1)
            if len(body) > 1024*1024:
                raise ValueError('Backend readiness response exceeds limit')
            return json.loads(body)
    get('/health')
    props = get('/props')
    if props.get('model_alias') != model:
        raise ValueError('Loaded model differs from the requested model')
    # Do not archive provider configuration, keys, or arbitrary response bodies.
    context_size = (props.get('default_generation_settings') or {}).get('n_ctx')
    return dict(model_alias=props['model_alias'],
                context_size=context_size if type(context_size) is int and context_size > 0 else None,
                chat_template_sha256=hashlib.sha256(props['chat_template'].encode()).hexdigest()
                if isinstance(props.get('chat_template'), str) else None)


def main():
    parser = build_parser()
    args = parser.parse_args()
    if not args.prepare_only and not args.allow_paid_live_inference:
        parser.error('Explicit --allow-paid-live-inference required; existing GPU only')
    cases, arms = args.cases.split(','), args.arms.split(',')
    if not cases or any(c not in {f'{i:02}' for i in range(1,21)} for c in cases) or len(set(cases)) != len(cases):
        parser.error('Select unique case IDs 01..20')
    if not arms or any(a not in {'fixed','entry','continuous','decision'} for a in arms) or len(set(arms)) != len(arms):
        parser.error('Select unique arms: fixed,entry,continuous,decision')
    args.output = args.output.resolve()
    plan = prepare_experiment(args, cases, arms)
    binary, cwd, cli = Path(plan['test_binary']), Path(plan['runtime_root']), Path(plan['poml_cli'])
    rows = load(args.output / 'rows.json', []) if args.resume else []
    requested = len(cases)*len(arms)
    if any((r['id'], r['arm']) not in {(case, arm) for case in cases for arm in arms} for r in rows):
        raise ValueError('Saved results do not match the frozen experiment plan')
    write_reports(args.output, rows, requested)
    if args.prepare_only:
        print(f'Prepared {requested} task runs; no inference started. Add --resume --allow-paid-live-inference to run.')
        return
    env = os.environ.copy()
    env.pop('GPU_ROUTER_URL', None)
    env.update(POML_CLI=str(cli), PRAXIS_LIVE_URL=args.url, PRAXIS_LIVE_MODEL=args.model, ROOT_DIR=str(cwd))
    env.pop('PRAXIS_DECISION_PROFILE', None)
    if 'decision' in arms:
        env['PRAXIS_DECISION_PROFILE'] = str(cwd / 'decisions/task-router.json')
    headers = {'Authorization': 'Bearer '+env['PRAXIS_LIVE_API_KEY']} if env.get('PRAXIS_LIVE_API_KEY') else {}
    endpoints = [('solver', args.url, args.model, headers)]
    if 'decision' in arms:
        profile = plan['decision_profile']
        endpoints.append(('decision', profile['endpoint'].rsplit('/', 1)[0].removesuffix('/v1'), profile['model'], {}))
    try:
        backends = {name: backend_ready(url, model, auth) for name, url, model, auth in endpoints}
    except Exception:
        (args.output / 'stopped.json').write_text(json.dumps({'phase':'preflight','reason':'Backend unavailable or wrong model; no inference started'}))
        raise SystemExit('Backend preflight failed; no inference started. See stopped.json.')
    previous = load(args.output / 'backends.json')
    if previous is not None and previous != backends:
        raise SystemExit('Backend model/template identity changed; start a fresh experiment.')
    (args.output / 'backends.json').write_text(json.dumps(backends, indent=2))
    completed = {(r['id'], r['arm']) for r in rows}
    for index, case in enumerate(cases):
        # Rotate arm order to reduce simple warm-cache/order bias. No claim of
        # controlled latency: other clients may still share the GPU.
        shift = index % len(arms)
        for arm in arms[shift:]+arms[:shift]:
            if (case, arm) in completed:
                continue
            try:
                for name, url, model, auth in endpoints:
                    if backend_ready(url, model, auth) != backends[name]:
                        raise ValueError('Backend changed')
                verify_snapshot(plan)
            except Exception:
                (args.output / 'stopped.json').write_text(json.dumps({'phase':'between_runs','case':case,'arm':arm,'reason':'Backend/runtime unavailable or changed'}))
                raise SystemExit('Experiment stopped before the next run; existing results preserved.')
            out = args.output / f'{case}-{arm}'
            task_env = dict(env, PRAXIS_LIVE_ARTIFACTS=str(out), PRAXIS_STATE_CASE=case, PRAXIS_STATE_ARM=arm)
            start = time.monotonic()
            with (args.output / f'{case}-{arm}.log').open('w') as log:
                try:
                    exit_status = subprocess.run([str(binary), 'local_model_live_state_task', '--ignored', '--nocapture', '--test-threads=1'],
                                                 cwd=cwd, env=task_env, stdout=log, stderr=subprocess.STDOUT, timeout=420).returncode
                except subprocess.TimeoutExpired:
                    exit_status = 124
            if not (out / 'fixture.json').exists():
                raise RuntimeError(f'Fixture failed before inference: {case}/{arm}; see its log')
            row = analyse_run(out)
            row.update(exit_status=exit_status, wall_elapsed_s=round(time.monotonic()-start,3))
            row['task_rubric_ok'] = row['task_rubric_ok'] and exit_status == 0
            rows.append(row)
            write_reports(args.output, rows, requested)
            print(f"{len(rows)}/{requested} {case}/{arm}: completed={row['completed']} rubric={row['task_rubric_ok']} model_switches={len(row['model_transitions'])} state={row['final_state']}", flush=True)
    verify_snapshot(plan)
    (args.output / 'stopped.json').unlink(missing_ok=True)


if __name__ == '__main__':
    main()
