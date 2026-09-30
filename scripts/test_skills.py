#!/usr/bin/env python3
"""Offline, real-CLI tests for shipped Praxis skills and POML authoring examples.

Run from any directory with --cli /path/to/poml/js/cli.js (or POML_CLI).
No providers, live databases, terminal sessions or services are touched.
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cli', default=os.environ.get('POML_CLI'))
    args = parser.parse_args()
    if not args.cli or not Path(args.cli).is_file():
        parser.error('Set --cli or POML_CLI to the installed Microsoft POML JavaScript CLI')
    cli = str(Path(args.cli).resolve())
    count = 0
    with tempfile.TemporaryDirectory(prefix='praxis-skills-test-') as folder:
        def render(path, context, expected=(), absent=(), fails=False):
            nonlocal count
            ctx = Path(folder) / 'context.json'
            ctx.write_text(json.dumps(context, ensure_ascii=False), encoding='utf-8')
            proc = subprocess.run(
                ['node', cli, '--file', str(path), '--context-file', str(ctx),
                 '--strict', '--speakerMode=false'],
                capture_output=True, text=True, timeout=60, cwd=folder)
            if fails:
                assert proc.returncode != 0, f'{path}: invalid input unexpectedly rendered'
            else:
                assert proc.returncode == 0, f'{path}: CLI failed: {proc.stderr[:1200]}'
                output = json.loads(proc.stdout)['messages']
                assert isinstance(output, str) and output.strip(), f'{path}: empty/non-text render'
                for text in expected:
                    assert text in output, f'{path}: missing {text!r}'
                for text in absent:
                    assert text not in output, f'{path}: unexpected {text!r}'
            count += 1
            print(f'PASS {path.name}: {"expected render failure" if fails else "strict render"}')

        # Test bundled tmux wrappers with a stub, never a real tmux server.
        fake_bin = Path(folder) / 'bin'
        fake_bin.mkdir()
        fake_tmux = fake_bin / 'tmux'
        fake_tmux.write_text('#!/bin/sh\nprintf "%s\\n" "$@" > "$TMUX_TEST_ARGS"\nexit "${TMUX_TEST_EXIT:-0}"\n')
        fake_tmux.chmod(0o755)
        arg_file = Path(folder) / 'tmux-args'
        for script, arguments, expected_args in [
            ('create_session.sh', ['skill-test'], ['new-session', '-d', '-s', 'skill-test']),
            ('kill_session.sh', ['skill-test'], ['kill-session', '-t', '=skill-test']),
            ('list_sessions.sh', [], ['list-sessions', '-F', '#{session_name}: #{session_windows} windows (#{session_id})']),
        ]:
            for status in [0, 42]:
                env = dict(os.environ, PATH=str(fake_bin) + os.pathsep + os.environ['PATH'],
                           TMUX_TEST_ARGS=str(arg_file), TMUX_TEST_EXIT=str(status))
                proc = subprocess.run(['bash', str(ROOT / 'skills/tmux/scripts' / script), *arguments],
                                      env=env, capture_output=True, text=True, timeout=10)
                assert proc.returncode == status, (script, proc.returncode, status)
                assert arg_file.read_text().splitlines() == expected_args
                count += 1
                print(f'PASS {script}: stubbed tmux exit {status}')

        samples = {
            'code_review': ('code', 'const café = "{{not_evaluated}}";', 'suggested fix'),
            'debug': ('error', 'TypeError: 日本語 {{not_evaluated}}', 'Root Cause'),
            'tmux': ('user_request', 'List sessions; do not run commands {{not_evaluated}}', 'tmux'),
            'poml_templates': ('user_request', 'Create a French tutor {{not_evaluated}}', 'update_template'),
            'skill_creator': ('user_request', 'Create a skill 日本語 {{not_evaluated}}', 'poml_templates'),
        }
        manifests = {}
        for name, (key, sample, instruction) in samples.items():
            base = ROOT / 'skills' / name
            manifest = json.loads((base / 'skill.json').read_text(encoding='utf-8'))
            assert manifest['name'] == name and manifest['description'].strip()
            assert key in manifest['required_parameters'], f'{name}: input contract missing'
            manifests[name] = manifest
            render(base / 'skill.poml', {key: sample}, (sample, instruction))
            render(base / 'skill.poml', {}, fails=True)

        authoring = ROOT / 'skills/poml_templates'
        render(authoring / 'skill.poml', {'user_request': 'Build a template'},
               ('{{', 'for="', '<let', '<include', 'typeof', 'loop.index',
                'https://microsoft.github.io/poml/stable/language/template/'))
        render(authoring / 'skill.poml', {'user_request': 'Create a skill', 'target_kind': 'skill'},
               ('staged skill package', 'Return to the calling skill'), ('Save with update_template', 'key settings.system_template'))
        render(ROOT / 'skills/skill_creator/skill.poml', {'user_request': 'Create a skill'},
               ('target_kind', 'references/authoring.md'), ('https://microsoft.github.io',))
        discovery = ROOT / 'templates/discovery/skills.poml'
        render(discovery, {}, ('search_skills', '"queries":[]', '"names":[]', '"limit":5'))
        example = authoring / 'examples/starter.poml'
        render(example, {}, ('You are a helpful assistant', '1. Be accurate', 'No current request supplied'))
        render(example, {'user_prompt': None, 'custom_data': None}, ('No current request supplied',))
        render(example, {'user_prompt': '', 'custom_data': {}}, ('No current request supplied',))
        render(example, {'user_prompt': 'Bonjour 日本語', 'custom_data': {'show_footer': True}},
               ('Bonjour 日本語', 'End of instructions'), ('No current request supplied',))
        render(example, {'user_prompt': 'Explain POML', 'custom_data': {'show_footer': False}},
               ('Explain POML',), ('End of instructions',))

        # Keep the runnable example in the authoring reference honest too.
        reference = (authoring / 'reference.md').read_text(encoding='utf-8')
        source = re.search(r'```xml\n(.*?)\n```', reference, re.S).group(1)
        documented = Path(folder) / 'documented.poml'
        documented.write_text(source, encoding='utf-8')
        render(documented, {}, ('Use the latest conversation message',))
        render(documented, {'user_prompt': 'Hello', 'custom_data': {'items': []}}, ('Hello',), ('1.',))
        render(documented, {'user_prompt': None, 'custom_data': None}, ('Use the latest conversation message',))
        render(documented, {'custom_data': {'items': ['first', 'second']}}, ('1. first', '2. second'))
        row = Path(folder) / 'row.poml'
        row.write_text('<poml><p>{{i}}/{{loop.index}}/{{loop.length}}/{{String(loop.first)}}/{{String(loop.last)}}</p></poml>')
        includes = Path(folder) / 'includes.poml'
        includes.write_text('<poml><include src="row.poml" for="i in [1,2]" /></poml>')
        render(includes, {}, ('1/0/2/true/false', '2/1/2/false/true'))

        # Current agent, legacy message handler and dashboard-like contexts.
        context = {'mode': 'agent', 'username': 'Tester', 'turn': 1,
                   'custom_data': {}, 'skills': list(manifests.values()),
                   'used_tools_history_size': 50}
        for extra in ({}, {'user_prompt': None}, {'user_prompt': '', 'tools': []},
                      {'user_prompt': 'Create a template', 'tools': [{'name': 'use_skill'}]}):
            render(ROOT / 'templates/system.poml', {**context, **extra}, ('poml_templates', 'use_skill'))
        render(ROOT / 'templates/system.poml', {**context, 'mode': 'chat'},
               ('chat mode',), ('call `use_skill`',))
        # A typo must fail rather than be counted as successful fallback output.
        broken = Path(folder) / 'broken.poml'
        broken.write_text('<poml><p>{{missing_required_identifier}}</p></poml>', encoding='utf-8')
        render(broken, {}, fails=True)

        # Exercise the shipped validation command, not just its underlying CLI.
        helper = authoring / 'scripts/validate.py'
        good_context = Path(folder) / 'good.json'
        good_context.write_text('{"user_prompt":"Helper test"}')
        bad_context = Path(folder) / 'bad.json'
        bad_context.write_text('not JSON')
        array_context = Path(folder) / 'array.json'
        array_context.write_text('[]')
        empty = Path(folder) / 'empty.poml'
        empty.write_text('<poml></poml>')
        for template, extra, expected_status in [
            (example, [], 0),
            (example, ['--context-file', str(good_context)], 0),
            (example, ['--context-file', str(bad_context)], 1),
            (example, ['--context-file', str(array_context)], 2),
            (example, ['--cli', str(Path(folder) / 'missing.js')], 2),
            (Path(folder) / 'missing.poml', [], 2),
            (broken, [], 1),
            (empty, [], 1),
        ]:
            proc = subprocess.run([sys.executable, str(helper), str(template), '--cli', cli, *extra],
                                  capture_output=True, text=True, timeout=60, cwd=folder)
            assert proc.returncode == expected_status, (template, extra, proc.returncode, proc.stderr)
            assert ('PASS:' in proc.stdout) == (expected_status == 0)
            count += 1
            print(f'PASS validate.py: {template.name}, expected exit {expected_status}')
    print(f'PASS: {count} CLI/helper checks; no API calls or live state changes.')


if __name__ == '__main__':
    main()
