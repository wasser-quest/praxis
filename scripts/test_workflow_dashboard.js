#!/usr/bin/env node
// Execute dashboard graph/audit/telemetry helpers with synthetic DOM storage.
// No browser session, network, credentials or provider calls.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const root = path.resolve(__dirname, '..');
const elements = new Map();
function element(id) {
    if (!elements.has(id)) elements.set(id, { innerHTML: '', textContent: '', value: '', title: '', dataset: {}, options: [], classList: { toggle() {}, contains() { return false; } }, setAttribute(key, value) { this[key] = value; }, querySelectorAll() { return []; }, replaceChildren() { this.innerHTML = ''; } });
    return elements.get(id);
}
const context = vm.createContext({ console, setInterval() {}, document: { getElementById: element, querySelectorAll: () => [], addEventListener() {} }, chatUserId: 'default', authToken: 'fixture', escapeHtml: value => String(value).replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;').replaceAll('"', '&quot;').replaceAll("'", '&#39;') });
vm.runInContext(fs.readFileSync(path.join(root, 'static/workflow-dashboard.js'), 'utf8'), context);
function run(code) { return vm.runInContext(code, context); }
const graph = { name: 'Test graph', workflow: 'test', start: 'route', active_state: 'route', history: [], nodes: ['route','a','b','c','d','e'].map(id => ({ id, title:id, description:'Purpose', variables:{}, decision_ir:{ N:'agent_next', K:'agent_back' } })), edges: ['a','b','c','d','e'].flatMap((to,index) => [{ id:`route:${index}`, index, from:'route', to, title:to, description:'Choose', kind:'transition', eligible:index !== 2, blocked_reason:index === 2 ? 'Tests required' : null }, { id:`${to}:0`, index:0, from:to, to:'route', title:'Return', kind:'transition', eligible:true }]) };
context.fixture = graph;
run('graphData = fixture; graphSelectedNode = "route"; renderWorkflowGraph(true); inspectGraphNode("route");');
assert.equal((element('workflow-graph').innerHTML.match(/data-node=/g) || []).length,6);
assert(element('graph-inspector').innerHTML.includes('Tests required'));
assert(element('graph-inspector').innerHTML.includes('2 · c'), 'blocked edge retains its index');
assert.equal(run('layoutWorkflowGraph(fixture).positions.size'),6, 'cycles must not hide nodes or loop');
const before = element('workflow-graph').viewBox;
run('zoomWorkflowGraph(0.8);'); assert.notEqual(element('workflow-graph').viewBox,before);
run('fitWorkflowGraph();'); assert.equal(element('workflow-graph').viewBox,before);
context.fixture.nodes[1].title = '<script>alert(1)</script>';
context.fixture.nodes[1].decision_ir = { R:'inspect_file' };
run('inspectGraphNode("a");');
assert(!element('graph-inspector').innerHTML.includes('<script>'));
assert(element('graph-inspector').innerHTML.includes('&lt;script&gt;'));
assert(element('graph-inspector').innerHTML.includes('inspect_file'));
assert(!element('graph-inspector').innerHTML.includes('agent_back'), 'inspector uses state-local IR');
assert.equal(run('diffPromptLines("a\\nb\\nc", "b\\na\\nc").removed.join("|")'),'a|b', 'reordered instructions count as changes');
run('updateHistoryBudget({history_tokens_estimate:6240, history_limit:32000, compaction_enabled:true,compaction_threshold:8000,compaction_due:false});');
assert.equal(element('history-progress').value,19.5);
assert(element('compaction-budget').textContent.includes('8,000'));
run('receiveGeneration({request_id:1,status:"started",attempt:1,output_limit:8192}); receiveGeneration({request_id:1,status:"generating",tokens_per_sec:12.5,estimated:true,first_token_ms:240});');
assert(element('generation-speed').textContent.includes('estimate'));
assert.equal(element('generation-latency').textContent,'First output 0.24s');
run('receiveGeneration({request_id:2,status:"completed",tokens_per_sec:99});');
assert(!element('generation-speed').textContent.includes('99'), 'late measurements cannot replace another request');
run('receiveGeneration({request_id:1,status:"completed",tokens_per_sec:null});');
assert(element('generation-speed').textContent.includes('unavailable'), 'missing usage is not zero tokens/s');
element('message-filter').value = 'all';
context.auditFixture = { user:'default', conversation:{messages:[],message_count:0}, audit:{limits:{history_tokens_estimate:50,history_limit:100,compaction_enabled:false}, events:[{id:1,kind:'model_call',message_cursor:0,created_at:'now',prompt_hash:'first',system_prompts:['Read source'],payload:{status:'completed',provider:'ollama',attempt:1,settings:{active_state:'read'}}},{id:2,kind:'model_call',message_cursor:0,created_at:'now',prompt_hash:'second',system_prompts:['Edit source'],payload:{status:'completed',provider:'ollama',attempt:1,settings:{active_state:'edit'}}}]} };
run('executionTimeline = auditFixture; renderExecutionTimeline();');
assert(element('messages-list').innerHTML.includes('Actual system prompts'));
assert(element('messages-list').innerHTML.includes('· changed'));
assert(element('messages-list').innerHTML.includes('1 settings'));
assert(element('messages-list').innerHTML.includes('Usage unavailable'));
element('message-filter').value = 'changes'; run('renderExecutionTimeline();');
assert(!element('messages-list').innerHTML.includes('MODEL CALL'));
(async () => {
    run('graphData = fixture; renderWorkflowGraph(true); inspectGraphNode("a")');
    element('graph-user-id').value = 'default';
    element('graph-workflow').value = '';
    context.apiGet = async () => ({ ok: false, status: 500, json: async () => ({ error: 'Temporary failure' }) });
    await run('loadGraphs()');
    assert.equal(element('workflow-graph').innerHTML, '');
    context.apiGet = async () => ({ ok: true, json: async () => graph });
    await run('loadGraphs()');
    assert.equal((element('workflow-graph').innerHTML.match(/data-node=/g) || []).length, 6, 'the same graph redraws after a transient error');

    run('updateHistoryBudget({history_tokens_estimate:50,history_limit:100,compaction_enabled:false})');
    const previousBudget = element('history-budget').textContent;
    context.apiGet = async () => ({ ok: true, json: async () => {
        context.chatUserId = 'another-context';
        return { history_tokens_estimate:99, history_limit:100, compaction_enabled:false };
    } });
    await run('loadChatBudget("default")');
    assert.equal(element('history-budget').textContent, previousBudget, 'a late usage response cannot replace another chat context');
    console.log('test_workflow_dashboard: graph cycles/indices, local IR, escaping, prompt changes, filters, metrics and async recovery passed');
})().catch(error => { console.error(error); process.exitCode = 1; });
