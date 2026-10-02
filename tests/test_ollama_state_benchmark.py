"""Ollama readiness must select an installed model without llama.cpp probes or inference."""
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
from unittest.mock import patch
import sys
import tempfile
import threading
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
import bench_state_machine as benchmark


@contextmanager
def ollama_server(*, digest='a'*64, version='0.35.0', capabilities=None, model='solver:latest'):
    requests = []
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_): pass
        def do_GET(self):
            requests.append((self.path, None))
            values = {'/api/tags': {'models': [{'name': model, 'model': model, 'digest': digest}]},
                      '/api/version': {'version': version}}
            self.respond(values.get(self.path))
        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            requests.append((self.path, body))
            self.respond({'template': 'template', 'parameters': 'num_ctx 4096', 'system': '',
                          'capabilities': capabilities or ['completion', 'tools']} if self.path == '/api/show' else None)
        def respond(self, body):
            self.send_response(200 if body is not None else 404)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps(body or {}).encode())
    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try: yield f'http://127.0.0.1:{server.server_port}', requests
    finally:
        server.shutdown(); server.server_close(); thread.join()


class OllamaBenchmarkTests(unittest.TestCase):
    def test_native_readiness_keeps_the_health_props_protocol(self):
        responses = [{'status':'ok'}, {'model_alias':'solver','chat_template':'template',
                       'default_generation_settings':{'n_ctx':4096}}]
        class Response:
            status=200
            def __init__(self, value): self.value=value
            def __enter__(self): return self
            def __exit__(self,*_): pass
            def read(self,_): return json.dumps(self.value).encode()
        with patch.object(benchmark.urllib.request,'urlopen',side_effect=[Response(v) for v in responses]) as request:
            identity=benchmark.backend_ready('http://native','solver')
            self.assertEqual(identity['model_alias'],'solver')
            self.assertEqual([c.args[0].full_url for c in request.call_args_list],['http://native/health','http://native/props'])

    def test_readiness_uses_tags_show_and_pins_selected_digest_without_inference(self):
        with ollama_server() as (url, requests):
            identity = benchmark.backend_ready(url, 'solver:latest', provider='ollama', require_tools=True)
            self.assertEqual(identity['model_digest'], 'a'*64)
            self.assertEqual(identity['model_alias'], 'solver:latest')
            self.assertEqual(requests, [('/api/tags', None), ('/api/show', {'model': 'solver:latest'})])

    def test_implicit_latest_tag_matches_the_installed_identity(self):
        with ollama_server() as (url, _):
            self.assertEqual(benchmark.backend_ready(url, 'solver', provider='ollama')['model_alias'], 'solver:latest')

    def test_exact_untagged_name_reported_by_tags_is_accepted(self):
        with ollama_server(model='solver') as (url, _):
            self.assertEqual(benchmark.backend_ready(url, 'solver', provider='ollama')['model_alias'], 'solver')

    def test_missing_model_fails_before_show_or_inference(self):
        with ollama_server() as (url, requests):
            with self.assertRaises(ValueError): benchmark.backend_ready(url, 'missing', provider='ollama')
            self.assertEqual(requests, [('/api/tags', None)])

    def test_solver_requires_tool_capability_when_the_server_reports_it(self):
        with ollama_server(capabilities=['completion']) as (url, _):
            with self.assertRaises(ValueError):
                benchmark.backend_ready(url, 'solver:latest', provider='ollama', require_tools=True)

    def test_systemone_requires_supported_ollama_version(self):
        for version, supported in [('0.34.9', False), ('0.35.0', True), ('0.35.1', True)]:
            with self.subTest(version=version), ollama_server(version=version) as (url, requests):
                if supported:
                    self.assertEqual(benchmark.backend_ready(url, 'solver:latest', provider='ollama', require_systemone=True)['ollama_version'], version)
                else:
                    with self.assertRaises(ValueError):
                        benchmark.backend_ready(url, 'solver:latest', provider='ollama', require_systemone=True)
                self.assertTrue(all(p in {'/api/tags','/api/show','/api/version'} for p,_ in requests))

    def test_model_digest_change_changes_backend_identity(self):
        with ollama_server(digest='a'*64) as (url, _): first=benchmark.backend_ready(url, 'solver:latest', provider='ollama')
        with ollama_server(digest='b'*64) as (url, _): second=benchmark.backend_ready(url, 'solver:latest', provider='ollama')
        self.assertNotEqual(first, second)

    def test_prepare_selects_both_models_on_one_url_and_preserves_probability_threshold(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d); source=root/'source'
            for name in ['contexts','templates','tests/fixtures','decisions']: (source/name).mkdir(parents=True)
            (source/'contexts/test.sm').write_text('state')
            (source/'templates/test.poml').write_text('template')
            (source/'tests/fixtures/20-tasks.json').write_text('[]')
            profile={'endpoint':'http://old/v1/decision','model':'old-router','instructions':'A: General work\nB: Debugging',
                     'schema':{'category':{'type':'enum','choices':['A','B']}},'state_field':'category',
                     'state_map':{'A':'standard','B':'debugger'},'minimum_probability':0.8,'timeout_ms':2000}
            (source/'decisions/task-router.json').write_text(json.dumps(profile))
            binary=root/'binary';binary.write_text('binary')
            cli=root/'cli';cli.write_text('cli')
            args=benchmark.build_parser().parse_args(['--test-binary',str(binary),'--runtime-root',str(source),
                '--poml-cli',str(cli),'--output',str(root/'output'),'--url','http://ollama:8080',
                '--provider','ollama','--model','solver:latest','--decision-model','nimble:latest','--prepare-only'])
            plan=benchmark.prepare_experiment(args,['01'],['fixed','decision'])
            self.assertEqual(plan['configuration']['provider'],'ollama')
            self.assertEqual(plan['configuration']['thinking_mode'],'auto')
            selected=plan['decision_profile']
            self.assertEqual(selected['backend'],'ollama')
            self.assertEqual(selected['endpoint'],'http://ollama:8080/v1/systemone')
            self.assertEqual(selected['model'],'nimble:latest')
            self.assertEqual(selected['minimum_probability'],0.8)
            self.assertEqual(selected['criteria'],{'A':'General work','B':'Debugging'})
            self.assertEqual(selected['timeout_ms'],60000)
            self.assertEqual(json.loads((source/'decisions/task-router.json').read_text()),profile)
            args.resume=True
            self.assertEqual(benchmark.prepare_experiment(args,['01'],['fixed','decision']),plan)
            args.decision_minimum_probability=0.9
            with self.assertRaises(ValueError): benchmark.prepare_experiment(args,['01'],['fixed','decision'])

    def test_classifier_probability_and_confidence_remain_distinct_in_scoring(self):
        from test_state_benchmark import StateScoringTests
        with tempfile.TemporaryDirectory() as d:
            root=Path(d)
            fixture=StateScoringTests()
            fixture.fixture(root,arm='decision',observed='debugger',final='debugger')
            fixture.route(root)
            trace=json.loads((root/'decision-routes.json').read_text())
            trace['events'][0]['route'].update(probability=0.91,confidence=0.1,usage={'input_tokens':25,'output_tokens':1})
            (root/'decision-routes.json').write_text(json.dumps(trace))
            row=benchmark.analyse_run(root)
            self.assertEqual(row['decision_probabilities'],[0.91])
            self.assertEqual(row['decision_confidences'],[0.1])
            self.assertEqual(row['decision_input_tokens'],25)
            self.assertEqual(row['decision_output_tokens'],1)
            trace['events'][0]['route'].pop('usage')
            (root/'decision-routes.json').write_text(json.dumps(trace))
            self.assertIsNone(benchmark.analyse_run(root)['decision_input_tokens'])


if __name__ == '__main__': unittest.main()
