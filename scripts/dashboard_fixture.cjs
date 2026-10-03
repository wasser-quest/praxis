// Browser review fixture: real dashboard assets, synthetic API data, no inference.
// Run from the repository root: node scripts/dashboard_fixture.cjs
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const root = path.resolve(__dirname, '..');
const limits = { history_tokens_estimate: 6240, history_limit: 32000, compaction_enabled: true, compaction_threshold: 8000, compaction_due: false, model_context_limit: null };
const ids = ['route', 'inspect', 'implement', 'build', 'test', 'review'];
const descriptions = ['Choose the next useful action.', 'Read the source and its current hash.', 'Apply a verified source modification.', 'Verify the workspace builds.', 'Run the regression tests.', 'Review current receipts before completion.'];
const graph = {
    name: 'Verified Rust workflow', workflow: 'branching-coding', description: 'Choose actions; Praxis verifies their effects.', routing: 'graph', start: 'route', active_state: 'implement', history: ['route', 'inspect', 'route'],
    nodes: ids.map((id, index) => ({ id, title: ['Choose a direction', 'Inspect source', 'Implement change', 'Build workspace', 'Run tests', 'Review evidence'][index], description: descriptions[index], variables: { 'settings.system_template': 'graph-coding' }, decision_ir: { ...(id === 'implement' ? { R: 'inspect_file', M: 'verified_rust/modify_source' } : id === 'inspect' ? { R: 'inspect_file' } : id === 'build' ? { B: 'verified_rust/build_workspace' } : id === 'test' ? { T: 'verified_rust/run_workspace_tests' } : {}), N: 'agent_next', K: 'agent_back' }, guards: [], action_guards: id === 'review' ? ['verified_rust/build_workspace', 'verified_rust/run_workspace_tests'] : [] })),
    edges: ids.slice(1).flatMap((id, index) => [{ id: `route:${index}`, index, from: 'route', to: id, title: graphTitle(index), description: descriptions[index + 1], condition: '', kind: 'transition', eligible: id !== 'review', blocked_reason: id === 'review' ? 'Current verified build and test receipts are required.' : null }, { id: `${id}:0`, index: 0, from: id, to: 'route', title: 'Choose another action', description: 'Return to the routing node.', condition: '', kind: 'transition', eligible: true, blocked_reason: null }])
};
function graphTitle(index) { return ['Inspect', 'Implement', 'Build', 'Test', 'Review'][index]; }
const prompts = ['You are Praxis. Inspect the existing source before editing.\nUse the current workflow graph and verified receipts.', 'You are Praxis. Apply a verified source modification.\nUse the current workflow graph and verified receipts.'];
const audit = {
    limits, events: [
        { id: 1, kind: 'model_call', message_cursor: 1, created_at: '2026-10-03T16:55:01Z', prompt_hash: 'inspect', system_prompts: [prompts[0]], payload: { provider: 'ollama', model: 'example-model', state: 'inspect', template: 'graph-coding', attempt: 1, status: 'completed', usage: { prompt_tokens: 1850, completion_tokens: 140 }, tokens_per_sec: 42.6, output_limit: 8192, elapsed_ms: 3286, first_token_ms: 240, limits, settings: { active_state: 'inspect', system_template: 'graph-coding' }, decision_ir: { R: 'inspect_file', N: 'agent_next', K: 'agent_back' }, tools: [] } },
        { id: 2, kind: 'state_transition', message_cursor: 2, created_at: '2026-10-03T16:55:05Z', payload: { from_state: 'inspect', to_state: 'route', back: true, verified: true, workflow: graph.workflow } },
        { id: 3, kind: 'model_call', message_cursor: 2, created_at: '2026-10-03T16:55:06Z', prompt_hash: 'implement', system_prompts: [prompts[1]], payload: { provider: 'ollama', model: 'example-model', state: 'implement', template: 'graph-coding', attempt: 1, status: 'completed', usage: { prompt_tokens: 2410, completion_tokens: 680 }, tokens_per_sec: 38.2, output_limit: 8192, elapsed_ms: 17801, first_token_ms: 190, limits, settings: { active_state: 'implement', system_template: 'graph-coding' }, decision_ir: graph.nodes[2].decision_ir, tools: [] } }
    ]
};
const messages = [{ id: 1, role: 'user', content: 'Implement Snake in the prepared Rust project.' }, { id: 2, role: 'tool', tool_name: 'execute_decision', content: JSON.stringify({ exists: true, sha256: 'abc123', content: 'fn main() {}' }) }, { id: 3, role: 'assistant', content: 'The source change committed. Next I will verify the build and tests.', prompt_tokens: 2410, completion_tokens: 680, total_tokens: 3090, generation_ms: 17801 }];
const ctx = { user_id: 'default', session_id: '', active_state: 'implement', settings: { active_state: 'implement', sm_file: graph.workflow, system_template: 'graph-coding', active_templates: [], compaction_enabled: true, history_token_limit: 32000 }, custom_data: {}, sm_data: {} };
const streams = new Set();
function emit(event, data) { for (const stream of streams) stream.write(`event: ${event}\ndata: ${JSON.stringify({ event, data: JSON.stringify(data) })}\n\n`); }
const server = http.createServer((request, response) => {
    const url = new URL(request.url, 'http://localhost');
    if (url.pathname.startsWith('/api/chat/stream/')) { response.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache' }); response.write(': fixture connected\n\n'); streams.add(response); request.on('close', () => streams.delete(response)); return; }
    let data;
    if (url.pathname === '/__fixture/generation') { emit('generation', { request_id: 4, status: 'started', attempt: 1, output_limit: 8192, limits }); emit('generation', { request_id: 4, status: 'generating', estimated: true, tokens_per_sec: 35.5, first_token_ms: 240 }); data = { ok: true }; }
    else if (url.pathname === '/api/contexts') data = { contexts: [{ user_id: 'default', data: ctx }] };
    else if (url.pathname.startsWith('/api/contexts/')) data = ctx;
    else if (url.pathname === '/api/status') data = { status: 'online', version: 'fixture' };
    else if (url.pathname === '/api/chat-sessions') data = { sessions: [{ user_id: 'default', title: 'Rust Snake', active_state: 'implement' }] };
    else if (url.pathname.startsWith('/api/graphs/')) data = graph;
    else if (url.pathname === '/api/sm-files') data = { files: [{ name: 'branching-coding.sm' }] };
    else if (url.pathname.startsWith('/api/execution/')) data = audit;
    else if (url.pathname.startsWith('/api/usage/')) data = limits;
    else if (url.pathname.startsWith('/api/messages/')) data = { messages, message_count: messages.length, total_tokens: 6240 };
    else if (url.pathname.startsWith('/api/agent/status/')) data = { active: false };
    else if (url.pathname.startsWith('/api/sm/')) data = { sm_file: graph.workflow, active_state: 'implement', active_templates: ['graph-coding'], sm_data: {} };
    else if (url.pathname === '/api/pairings') data = { pairings: [] };
    else if (url.pathname === '/api/pairings/pending') data = { pending: [] };
    else if (url.pathname.startsWith('/api/')) data = {};
    if (data != null) { response.writeHead(200, { 'Content-Type': 'application/json' }); response.end(JSON.stringify(data)); return; }
    const relative = url.pathname === '/' ? 'static/index.html' : url.pathname === '/logo.svg' ? 'static/logo.svg' : url.pathname.replace(/^\//, '');
    const filename = path.resolve(root, relative);
    if (!filename.startsWith(root + path.sep) || !fs.existsSync(filename)) { response.writeHead(404); response.end(); return; }
    const type = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml' }[path.extname(filename)] || 'application/octet-stream';
    let body = fs.readFileSync(filename);
    if (relative === 'static/index.html') body = body.toString().replace('<script src="/static/app.js', '<script>localStorage.setItem("praxis_token","dashboard-fixture");</script><script src="/static/app.js');
    response.writeHead(200, { 'Content-Type': type }); response.end(body);
});
server.listen(44148, '127.0.0.1', () => console.log('Dashboard fixture: http://127.0.0.1:44148'));
