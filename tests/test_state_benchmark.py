"""Scoring regressions: a claimed persona is not an executed state transition."""
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
from bench_state_machine import analyse_run, compare_arms, write_reports
import bench_state_machine as benchmark


class StateScoringTests(unittest.TestCase):
    def fixture(self, root, *, call=False, receipt=False, observed='standard', final='standard', arm='continuous'):
        task = {'id': '01', 'initial_state': 'standard', 'acceptable_states': ['debugger'], 'checks': ['cause']}
        def save(name, value):
            (root / name).write_text(json.dumps(value))
        save('fixture.json', {'task': task, 'arm': arm})
        tc = {'id': 'change', 'function': {'name': 'set_context', 'arguments': '{"key":"active_state","value":"debugger"}'}}
        save('call-00.request.json', {'observed_state': {'active_state': observed}})
        save('call-00.response.json', {'response': {'content': 'I am now a debugger. cause', 'tool_calls': [tc] if call else None}})
        history = [{'role': 'tool', 'tool_call_id': 'change', 'tool_name': 'set_context', 'content': "Context key 'active_state' set"}] if receipt else []
        save('result.json', {'http_result': {'success': True, 'response': 'cause'}, 'final_context': {'active_state': final}, 'history': history})

    def test_prose_and_attempt_without_receipt_are_not_transitions(self):
        for call in [False, True]:
            with tempfile.TemporaryDirectory() as d:
                root = Path(d)
                self.fixture(root, call=call)
                row = analyse_run(root)
                self.assertEqual(row['model_transitions'], [])
                self.assertTrue(row['task_rubric_ok'])
                self.assertFalse(row['appropriate_state_used'])

    def test_receipt_and_observed_context_confirm_a_real_transition(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            self.fixture(root, call=True, receipt=True, final='debugger')
            (root / 'call-01.request.json').write_text(json.dumps({'observed_state': {'active_state': 'debugger'}}))
            (root / 'call-01.response.json').write_text(json.dumps({'response': {'content': 'cause'}}))
            row = analyse_run(root)
            self.assertEqual(len(row['model_transitions']), 1)
            self.assertEqual(row['deterministic_transitions'], [])
            self.assertTrue(row['appropriate_state_used'])
            self.assertTrue(row['policy_followed'])

    def test_automatic_routing_is_separate_from_model_selection(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            self.fixture(root, observed='debugger', final='debugger')
            row = analyse_run(root)
            self.assertEqual(row['model_transitions'], [])
            self.assertEqual(len(row['deterministic_transitions']), 1)

    def test_error_receipt_does_not_count(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            self.fixture(root, call=True)
            result = json.loads((root / 'result.json').read_text())
            result['history'] = [{'role': 'tool', 'tool_call_id': 'change', 'content': 'Error: undefined state'}]
            (root / 'result.json').write_text(json.dumps(result))
            self.assertEqual(analyse_run(root)['model_transitions'], [])

    def test_more_switches_are_not_automatically_a_benefit(self):
        rows = [{'id': '01', 'arm': arm, 'task_rubric_ok': True, 'appropriate_state_used': arm == 'continuous',
                 'prompt_tokens': 100 if arm == 'fixed' else 200, 'completion_tokens': 10, 'elapsed_s': 1.0}
                for arm in ['fixed', 'continuous']]
        result = compare_arms(rows, 'fixed', 'continuous')
        self.assertEqual(result['rubric_improved_tasks'], 0)
        self.assertEqual(result['rubric_equal_tasks'], 1)
        self.assertEqual(result['prompt_token_delta'], 100)

    def route(self, root, *, status='applied', before_call=0, source='standard', target='debugger'):
        (root / 'decision-routes.json').write_text(json.dumps({
            'complete': True, 'events': [{'before_call': before_call, 'route': {
                'source': 'decision', 'status': status, 'profile': 'task-router',
                'from_state': source, 'to_state': target, 'elapsed_ms': 12,
            }}],
        }))

    def test_decision_applied_route_is_not_a_model_or_other_transition(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            self.fixture(root, observed='debugger', final='debugger', arm='decision')
            self.route(root)
            row = analyse_run(root)
            self.assertEqual(row['model_transitions'], [])
            self.assertEqual(row['deterministic_transitions'], [])
            self.assertEqual(len(row['decision_transitions']), 1)
            self.assertEqual(row['decision_calls'], 1)
            self.assertEqual(row['decision_elapsed_ms'], 12)
            self.assertTrue(row['routing_trace_complete'])
            self.assertTrue(row['policy_followed'])

    def test_low_confidence_or_failed_decision_does_not_count_as_applied(self):
        for status in ['low_probability', 'failed', 'unchanged', 'stale_context']:
            with tempfile.TemporaryDirectory() as d:
                root = Path(d)
                self.fixture(root, arm='decision')
                self.route(root, status=status, target='standard')
                row = analyse_run(root)
                self.assertEqual(row['decision_transitions'], [])
                self.assertEqual(row['decision_statuses'], {status: 1})

    def test_missing_or_lost_router_trace_cannot_establish_decision_benefit(self):
        for lost in [False, True]:
            with tempfile.TemporaryDirectory() as d:
                root = Path(d)
                self.fixture(root, observed='debugger', final='debugger', arm='decision')
                if lost:
                    self.route(root)
                    data = json.loads((root / 'decision-routes.json').read_text())
                    data['complete'] = False
                    (root / 'decision-routes.json').write_text(json.dumps(data))
                row = analyse_run(root)
                self.assertFalse(row['routing_trace_complete'])
                self.assertFalse(row['policy_followed'])

    def test_each_step_router_requires_an_event_for_each_solving_request(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            self.fixture(root, arm='decision')
            self.route(root, status='unchanged', target='standard')
            (root / 'call-01.request.json').write_text(json.dumps({'observed_state': {'active_state': 'standard'}}))
            (root / 'call-01.response.json').write_text(json.dumps({'response': {'content': 'cause'}}))
            self.assertFalse(analyse_run(root)['routing_trace_complete'])

    def test_applied_router_event_requires_matching_observed_state(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            self.fixture(root, arm='decision')
            self.route(root)
            row = analyse_run(root)
            self.assertFalse(row['routing_trace_complete'])
            self.assertFalse(row['policy_followed'])

    def test_model_state_write_violates_decision_policy(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            self.fixture(root, call=True, receipt=True, final='debugger', arm='decision')
            self.route(root, status='unchanged', target='standard')
            row = analyse_run(root)
            self.assertFalse(row['policy_followed'])
            self.assertEqual(len(row['model_transitions']), 1)

    def test_success_receipt_must_match_tool_name_and_fixture_path(self):
        for mismatch in ['name', 'path', 'error']:
            with tempfile.TemporaryDirectory() as d:
                root = Path(d)
                self.fixture(root)
                fixture = json.loads((root / 'fixture.json').read_text())
                fixture['task']['fixture'] = 'cause'
                fixture['fixture_path'] = '/synthetic/task.txt'
                (root / 'fixture.json').write_text(json.dumps(fixture))
                call = {'id': 'read', 'function': {'name': 'read_file', 'arguments': json.dumps({'path': '/other' if mismatch == 'path' else '/synthetic/task.txt'})}}
                (root / 'call-00.response.json').write_text(json.dumps({'response': {'tool_calls': [call]}}))
                result = json.loads((root / 'result.json').read_text())
                result['history'] = [{'role': 'tool', 'tool_call_id': 'read', 'tool_name': 'get_context' if mismatch == 'name' else 'read_file', 'content': json.dumps({'error': 'unreadable'}) if mismatch == 'error' else 'cause'}]
                (root / 'result.json').write_text(json.dumps(result))
                row = analyse_run(root)
                self.assertFalse(row['evidence_read'])
                self.assertFalse(row['task_rubric_ok'])

    def test_router_switch_after_confirmed_file_read_is_tracked_separately(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            self.fixture(root, arm='decision', final='debugger')
            fixture = json.loads((root / 'fixture.json').read_text())
            fixture['task']['fixture'] = 'cause'
            fixture['fixture_path'] = '/synthetic/task.txt'
            (root / 'fixture.json').write_text(json.dumps(fixture))
            (root / 'call-00.response.json').write_text(json.dumps({'response': {'tool_calls': [{'id': 'read', 'function': {'name': 'read_file', 'arguments': '{"path":"/synthetic/task.txt"}'}}]}}))
            result = json.loads((root / 'result.json').read_text())
            result['history'] = [{'role': 'tool', 'tool_call_id': 'read', 'tool_name': 'read_file', 'content': 'cause'}]
            (root / 'result.json').write_text(json.dumps(result))
            (root / 'call-01.request.json').write_text(json.dumps({'observed_state': {'active_state': 'debugger'}}))
            (root / 'call-01.response.json').write_text(json.dumps({'response': {'content': 'cause'}}))
            self.route(root, before_call=1)
            trace = json.loads((root / 'decision-routes.json').read_text())
            trace['events'].insert(0, {'before_call': 0, 'route': {
                'source': 'decision', 'status': 'unchanged', 'profile': 'task-router',
                'from_state': 'standard', 'to_state': 'standard', 'elapsed_ms': 12,
            }})
            (root / 'decision-routes.json').write_text(json.dumps(trace))
            row = analyse_run(root)
            self.assertTrue(row['evidence_read'])
            self.assertTrue(row['appropriate_state_after_evidence'])
            self.assertTrue(row['decision_transitions'][0]['after_evidence_read'])
            self.assertEqual(row['deterministic_transitions'], [])

    def test_duplicate_task_arm_is_rejected_instead_of_overwriting_a_run(self):
        row = {'id': '01', 'arm': 'fixed', 'task_rubric_ok': True, 'appropriate_state_used': False, 'prompt_tokens': 1, 'completion_tokens': 1, 'elapsed_s': 1}
        with self.assertRaises(ValueError):
            compare_arms([row, row], 'fixed', 'decision')

    def test_partial_report_and_state_switches_do_not_claim_task_benefit(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            self.fixture(root, arm='decision')
            self.route(root, status='unchanged', target='standard')
            row = analyse_run(root)
            report = write_reports(root, [row], requested=80)
            self.assertEqual(report['experiment_status'], 'incomplete')
            self.assertEqual(report['decision_conclusion'], 'insufficient_evidence')

    def test_decision_comparison_keeps_quality_and_policy_separate(self):
        rows = [{'id': '01', 'arm': arm, 'task_rubric_ok': arm == 'decision', 'policy_followed': arm == 'fixed',
                 'appropriate_state_used': arm == 'decision', 'prompt_tokens': 100, 'completion_tokens': 10, 'elapsed_s': 1.0}
                for arm in ['fixed', 'decision']]
        result = compare_arms(rows, 'fixed', 'decision')
        self.assertEqual(result['rubric_improved_tasks'], 1)
        self.assertEqual(result['compliant_rubric_improved_tasks'], 0)

    def test_parser_includes_decision_arm_and_offline_preparation(self):
        args = benchmark.build_parser().parse_args([
            '--test-binary', '/tmp/tests', '--url', 'http://127.0.0.1:1',
            '--model', 'solver', '--poml-cli', '/tmp/poml', '--output', '/tmp/result', '--prepare-only',
        ])
        self.assertEqual(args.arms, 'fixed,entry,continuous,decision')
        self.assertTrue(args.prepare_only)

    def test_prepare_freezes_inputs_and_profile_without_inference(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            source = root / 'source'
            for name in ['contexts', 'templates', 'tests/fixtures', 'decisions']:
                (source / name).mkdir(parents=True)
            (source / 'contexts/20-tasks.sm').write_text('state source')
            (source / 'templates/20-tasks.poml').write_text('<poml>source</poml>')
            (source / 'tests/fixtures/20-tasks.json').write_text('[]')
            profile = {'endpoint': 'http://127.0.0.1:2/v1/decision', 'model': 'classifier'}
            (source / 'decisions/task-router.json').write_text(json.dumps(profile))
            binary, cli = root / 'tests-bin', root / 'poml-cli'
            binary.write_text('binary'); cli.write_text('renderer')
            args = benchmark.build_parser().parse_args([
                '--test-binary', str(binary), '--runtime-root', str(source), '--url', 'http://127.0.0.1:1',
                '--model', 'solver', '--poml-cli', str(cli), '--output', str(root / 'output'),
                '--cases', '01', '--decision-url', 'http://127.0.0.1:3/v1/decision', '--prepare-only',
            ])
            plan = benchmark.prepare_experiment(args, ['01'], ['fixed', 'decision'])
            self.assertEqual(plan['requested_runs'], 2)
            self.assertEqual(plan['decision_profile']['endpoint'], 'http://127.0.0.1:3/v1/decision')
            self.assertEqual(json.loads((source / 'decisions/task-router.json').read_text()), profile)
            frozen = Path(plan['runtime_root'])
            self.assertEqual((frozen / 'scripts/bench_state_machine.py').read_bytes(), Path(benchmark.__file__).read_bytes())
            args.resume = True
            self.assertEqual(benchmark.prepare_experiment(args, ['01'], ['fixed', 'decision']), plan)
            (source / 'contexts/20-tasks.sm').write_text('changed source')
            self.assertEqual((frozen / 'contexts/20-tasks.sm').read_text(), 'state source')
            benchmark.verify_snapshot(plan)
            (frozen / 'contexts/20-tasks.sm').write_text('tampered snapshot')
            with self.assertRaises(ValueError):
                benchmark.verify_snapshot(plan)

    def test_missing_usage_is_unknown_cost_not_zero_tokens(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            self.fixture(root)
            row = analyse_run(root)
            self.assertFalse(row['usage_complete'])
            self.assertIsNone(row['prompt_tokens'])
            self.assertIsNone(row['completion_tokens'])
            report = write_reports(root, [row], 1)
            self.assertIsNone(report['arms']['continuous']['prompt_tokens'])

    def test_archived_runtime_output_retains_exact_file_evidence(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            self.fixture(root)
            fixture = json.loads((root / 'fixture.json').read_text())
            fixture['task']['fixture'] = 'cause'
            fixture['fixture_path'] = '/synthetic/task.txt'
            (root / 'fixture.json').write_text(json.dumps(fixture))
            (root / 'call-00.response.json').write_text(json.dumps({'response': {'tool_calls': [
                {'id': 'read', 'function': {'name': 'read_file', 'arguments': '{"path":"/synthetic/task.txt"}'}}
            ]}}))
            result = json.loads((root / 'result.json').read_text())
            result['history'] = [{'role': 'tool', 'tool_call_id': 'read', 'tool_name': 'read_file',
                'content': 'cause\n\n[Saved tool response] output_id=fixture \n5 source characters; 5 / 5 bytes retained; storage_truncated=false. Use read_tool_result to inspect sections without re-execution. Retained up to 7 days, subject to quotas and actual history/session deletion.'}]
            (root / 'result.json').write_text(json.dumps(result))
            self.assertTrue(analyse_run(root)['evidence_read'])
            result['history'][0]['content'] = result['history'][0]['content'].replace('5 / 5', '4 / 5')
            (root / 'result.json').write_text(json.dumps(result))
            self.assertFalse(analyse_run(root)['evidence_read'])

    def test_missing_latency_is_unknown_and_excluded_from_pairs(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            self.fixture(root)
            row = analyse_run(root)
            self.assertIsNone(row['elapsed_s'])
            report = write_reports(root, [row], 1)
            self.assertIsNone(report['arms']['continuous']['median_elapsed_s'])
            paired = compare_arms([dict(row, arm='fixed'), dict(row, arm='decision', elapsed_s=1)], 'fixed', 'decision')
            self.assertEqual(paired['latency_paired_tasks'], 0)
            self.assertIsNone(paired['elapsed_s_delta'])

    def test_fixed_arm_rejects_failed_model_state_attempt_and_unexplained_drift(self):
        for call, observed in [(True, 'standard'), (False, 'debugger')]:
            with tempfile.TemporaryDirectory() as d:
                root = Path(d)
                self.fixture(root, arm='fixed', call=call, observed=observed, final=observed)
                self.assertFalse(analyse_run(root)['policy_followed'])

    def test_complete_scripted_corpus_still_has_no_live_conclusion(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            self.fixture(root, arm='decision')
            self.route(root, status='unchanged', target='standard')
            row = analyse_run(root)
            rows = [dict(row, id=f'{number:02}', arm=arm, inference_kind='scripted')
                    for number in range(1, 21) for arm in ['fixed', 'decision']]
            report = write_reports(root, rows, 40)
            self.assertEqual(report['experiment_status'], 'complete')
            self.assertEqual(report['decision_conclusion'], 'insufficient_evidence')

    def test_live_conclusion_requires_all_original_tasks_and_four_arms(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            self.fixture(root, arm='decision')
            self.route(root, status='unchanged', target='standard')
            row = analyse_run(root)
            rows = [dict(row, id=f'{number:02}', arm=arm, inference_kind='live')
                    for number in range(1, 21) for arm in ['fixed', 'entry', 'continuous', 'decision']]
            report = write_reports(root, rows, 80)
            self.assertEqual(report['decision_conclusion'], 'no_net_compliant_rubric_gain_on_this_corpus')
            two_arms = [r for r in rows if r['arm'] in {'fixed', 'decision'}]
            self.assertEqual(write_reports(root, two_arms, 40)['decision_conclusion'], 'insufficient_evidence')
            wrong_ids = [dict(r, id='21') if r['id'] == '20' else r for r in rows]
            self.assertEqual(write_reports(root, wrong_ids, 80)['decision_conclusion'], 'insufficient_evidence')


if __name__ == '__main__':
    unittest.main()
