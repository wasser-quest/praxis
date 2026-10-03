/* Runtime graphs, request audit and generation metrics. No separate .sm parser. */
let graphData = null;
let graphSelectedNode = null;
let graphLoadGeneration = 0;
let graphTopology = '';
let graphBox = { x: 0, y: 0, width: 1000, height: 600 };
let graphExtent = { width: 1000, height: 600 };
let graphMoved = false;
let executionLoadGeneration = 0;
let executionTimeline = null;
let activeGenerationRequest = null;
let generationContext = null;
let graphPressedNode = null;

const formatCount = value => value != null && Number.isFinite(Number(value)) ? Number(value).toLocaleString() : '—';

function updateHistoryBudget(limits) {
    if (!limits) return;
    const used = Number(limits.history_tokens_estimate || 0);
    const limit = Number(limits.history_limit || 0);
    document.getElementById('history-budget').textContent = `History ≈ ${formatCount(used)} / ${formatCount(limit)}`;
    const compaction = limits.compaction_enabled
        ? `Compaction at ≈ ${formatCount(limits.compaction_threshold)}${limits.compaction_due ? ' · due before next call' : ''}`
        : 'Compaction off';
    document.getElementById('compaction-budget').textContent = compaction;
    const progress = document.getElementById('history-progress');
    progress.value = limit > 0 ? Math.min(100, used / limit * 100) : 0;
    progress.classList.toggle('budget-high', progress.value >= 80);
}

async function loadChatBudget(uid = chatUserId) {
    if (generationContext !== uid) {
        generationContext = uid; activeGenerationRequest = null;
        document.getElementById('generation-speed').textContent = 'Generation —';
        document.getElementById('generation-latency').textContent = 'First output —';
    }
    try {
        const response = await apiGet(`/api/usage/${encodeURIComponent(uid)}`);
        if (response.ok) {
            const limits = await response.json();
            if (uid === chatUserId && generationContext === uid) updateHistoryBudget(limits);
        }
    } catch (_) { /* Existing metrics survive a transient connection failure. */ }
}

function receiveGeneration(data) {
    if (data.status === 'started') {
        activeGenerationRequest = data.request_id;
        document.getElementById('generation-speed').textContent = `Generating · attempt ${data.attempt}`;
        document.getElementById('generation-latency').textContent = 'Waiting for first output…';
        updateHistoryBudget(data.limits);
        document.getElementById('chat-telemetry').title = `Output allowance: ${formatCount(data.output_limit)} tokens. Request reservation estimate including output: ${formatCount(data.request_tokens_estimate)}. Model context window: unavailable.`;
        return;
    }
    if (data.request_id !== activeGenerationRequest) return;
    const speed = Number(data.tokens_per_sec);
    document.getElementById('generation-speed').textContent = data.status === 'failed'
        ? 'Generation interrupted'
        : Number.isFinite(speed) && data.tokens_per_sec != null
            ? `${data.estimated ? '≈ ' : ''}${speed.toFixed(1)} tokens/s${data.estimated ? ' · estimate' : ' · reported usage'}`
            : `${data.status} · usage unavailable`;
    if (data.first_token_ms != null) document.getElementById('generation-latency').textContent = `First output ${(data.first_token_ms / 1000).toFixed(2)}s`;
}

function attachMessageUsage(div, message) {
    if (!div || message.total_tokens == null) return;
    let badge = div.querySelector('.saved-usage');
    if (!badge) { badge = document.createElement('small'); badge.className = 'saved-usage'; div.append(badge); }
    const speed = message.generation_ms > 0 ? ` · ${(message.completion_tokens / (message.generation_ms / 1000)).toFixed(1)} tokens/s` : '';
    badge.textContent = `${formatCount(message.prompt_tokens)} in · ${formatCount(message.completion_tokens)} out${speed}`;
}

async function initGraphs() {
    await populateUserDropdowns();
    const select = document.getElementById('graph-user-id');
    if (!select.value && [...select.options].some(option => option.value === chatUserId)) select.value = chatUserId;
    try {
        const response = await apiGet('/api/sm-files');
        if (response.ok) {
            const data = await response.json();
            const files = data.files || data.sm_files || [];
            const current = document.getElementById('graph-workflow').value;
            document.getElementById('graph-workflow').innerHTML = '<option value="">Active workflow</option>' + files.map(file => `<option value="${escapeHtml(file.name)}">${escapeHtml(file.name)}</option>`).join('');
            document.getElementById('graph-workflow').value = current;
        }
    } catch (_) { /* The active workflow can still be loaded. */ }
    if (select.value) await loadGraphs();
}

async function loadGraphs() {
    const user = document.getElementById('graph-user-id').value;
    if (!user) return;
    const generation = ++graphLoadGeneration;
    const workflow = document.getElementById('graph-workflow').value;
    try {
        const response = await apiGet(`/api/graphs/${encodeURIComponent(user)}${workflow ? '?workflow=' + encodeURIComponent(workflow) : ''}`);
        const data = await response.json();
        if (generation !== graphLoadGeneration || user !== document.getElementById('graph-user-id').value) return;
        if (!response.ok) throw new Error(data.error || `Graph unavailable (${response.status})`);
        graphData = data;
        const topology = JSON.stringify([data.workflow, data.nodes.map(node => node.id), data.edges.map(edge => [edge.id, edge.from, edge.to])]);
        const changed = topology !== graphTopology;
        graphTopology = topology;
        if (!data.nodes.some(node => node.id === graphSelectedNode)) graphSelectedNode = data.active_state || data.nodes[0]?.id;
        document.getElementById('graph-status').textContent = `${data.name || data.workflow} · ${data.routing} · ${data.nodes.length} states · ${data.edges.length} connections${data.preview ? ' · preview' : ' · current: ' + (data.active_state || 'unset')}${data.history_source ? ' · last completed route' : ''}`;
        renderWorkflowGraph(changed);
        inspectGraphNode(graphSelectedNode);
    } catch (error) {
        if (generation !== graphLoadGeneration) return;
        document.getElementById('graph-status').textContent = error.message;
        graphData = null; graphTopology = '';
        const svg = document.getElementById('workflow-graph');
        svg.replaceChildren(); svg.graphMarkup = null;
        const inspector = document.getElementById('graph-inspector');
        inspector.textContent = 'Fix the workflow error, then refresh.'; inspector.graphMarkup = null;
    }
}

function layoutWorkflowGraph(graph) {
    const ranks = new Map();
    const start = graph.start || graph.active_state || graph.nodes[0]?.id;
    if (start) ranks.set(start, 0);
    const queue = start ? [start] : [];
    while (queue.length) {
        const current = queue.shift();
        for (const edge of graph.edges.filter(edge => edge.from === current && edge.kind !== 'auto')) {
            if (!ranks.has(edge.to)) { ranks.set(edge.to, ranks.get(current) + 1); queue.push(edge.to); }
        }
    }
    const last = Math.max(0, ...ranks.values()) + 1;
    const levels = new Map();
    for (const node of graph.nodes) {
        const rank = ranks.get(node.id) ?? last;
        if (!levels.has(rank)) levels.set(rank, []);
        levels.get(rank).push(node);
    }
    const positions = new Map();
    let y = 60, width = 480;
    for (const [, nodes] of [...levels].sort((a, b) => a[0] - b[0])) {
        nodes.forEach((node, index) => positions.set(node.id, { x: 60 + (index % 5) * 250, y: y + Math.floor(index / 5) * 145 }));
        width = Math.max(width, 120 + Math.min(5, nodes.length) * 250);
        y += Math.ceil(nodes.length / 5) * 145 + 50;
    }
    return { positions, width, height: Math.max(400, y + 30) };
}

function renderWorkflowGraph(fit = false) {
    if (!graphData) return;
    const layout = layoutWorkflowGraph(graphData);
    graphExtent = { width: layout.width, height: layout.height };
    const route = [...(graphData.history || []), graphData.active_state];
    const visited = new Set(route);
    const svg = document.getElementById('workflow-graph');
    const paths = graphData.edges.map(edge => {
        const a = layout.positions.get(edge.from), b = layout.positions.get(edge.to);
        if (!a || !b) return '';
        let path, labelX, labelY;
        if (edge.from === edge.to) {
            path = `M ${a.x + 210} ${a.y + 20} C ${a.x + 270} ${a.y - 45}, ${a.x + 285} ${a.y + 100}, ${a.x + 210} ${a.y + 65}`;
            labelX = a.x + 262; labelY = a.y + 20;
        } else {
            const x1 = a.x + 105, y1 = a.y + (b.y > a.y ? 82 : 0), x2 = b.x + 105, y2 = b.y + (b.y > a.y ? 0 : 82);
            const offset = Math.max(45, Math.abs(y2 - y1) / 2);
            const side = b.y > a.y ? 1 : -1;
            path = `M ${x1} ${y1} C ${x1 + (side < 0 ? 90 : 0)} ${y1 + side * offset}, ${x2 + (side < 0 ? 90 : 0)} ${y2 - side * offset}, ${x2} ${y2}`;
            labelX = (x1 + x2) / 2 + (side < 0 ? 65 : 0); labelY = (y1 + y2) / 2 - 7;
        }
        const traversed = route.some((state, index) => state === edge.from && route[index + 1] === edge.to);
        const label = edge.index == null ? edge.kind : `${edge.index} · ${edge.title}`;
        return `<g class="graph-edge ${edge.eligible ? '' : 'blocked'} ${traversed ? 'traversed' : ''}" data-kind="${escapeHtml(edge.kind)}"><title>${escapeHtml(edge.description || edge.title)}${edge.blocked_reason ? '\n' + escapeHtml(edge.blocked_reason) : ''}</title><path d="${path}" marker-end="url(#workflow-arrow)"/><text x="${labelX}" y="${labelY}" text-anchor="middle">${escapeHtml(label.length > 27 ? label.slice(0, 25) + '…' : label)}</text></g>`;
    }).join('');
    const nodes = graphData.nodes.map(node => {
        const p = layout.positions.get(node.id);
        const title = node.title.length > 24 ? node.title.slice(0, 22) + '…' : node.title;
        return `<g class="graph-node ${node.id === graphData.active_state ? 'current' : ''} ${visited.has(node.id) ? 'visited' : ''} ${node.id === graphSelectedNode ? 'selected' : ''}" transform="translate(${p.x},${p.y})" data-node="${escapeHtml(node.id)}" role="button" tabindex="0" aria-label="Inspect ${escapeHtml(node.title)}"><title>${escapeHtml(node.description || node.title)}</title><rect width="210" height="82" rx="10"/><circle cx="17" cy="21" r="4"/><text x="29" y="26" class="node-title">${escapeHtml(title)}</text><text x="17" y="46" class="node-id">${escapeHtml(node.id)}</text><text x="17" y="65" class="node-ir">IR ${escapeHtml(Object.keys(node.decision_ir || {}).sort().join(' · ') || 'disabled')}</text></g>`;
    }).join('');
    const markup = `<title>${escapeHtml(graphData.name || graphData.workflow)} state graph</title><defs><marker id="workflow-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z"/></marker></defs>${paths}${nodes}`;
    if (svg.graphMarkup !== markup) { svg.innerHTML = markup; svg.graphMarkup = markup; }
    if (fit) fitWorkflowGraph(); else applyGraphBox();
}

function inspectGraphNode(id) {
    if (!graphData) return;
    const node = graphData.nodes.find(node => node.id === id);
    if (!node) return;
    graphSelectedNode = id;
    document.querySelectorAll('.graph-node').forEach(element => element.classList.toggle('selected', element.dataset.node === id));
    const outgoing = graphData.edges.filter(edge => edge.from === id);
    const mappings = Object.entries(node.decision_ir || {}).sort(([a], [b]) => a.localeCompare(b));
    const inspector = document.getElementById('graph-inspector');
    const markup = `<span class="eyebrow">STATE INSPECTOR</span><h3>${escapeHtml(node.title)}</h3><code>${escapeHtml(id)}</code><p>${escapeHtml(node.description || 'No purpose declared. Add a [node ' + id + '] description to explain this state.')}</p>
        <h4>Decision IR</h4>${mappings.length ? `<dl class="ir-table">${mappings.map(([op, target]) => `<dt>${escapeHtml(op)}</dt><dd>${escapeHtml(target)}</dd>`).join('')}</dl>` : '<p>IR disabled in this state.</p>'}
        <h4>Outgoing choices</h4>${outgoing.length ? outgoing.map(edge => `<div class="edge-choice"><div><strong>${edge.index == null ? escapeHtml(edge.kind) : edge.index} · ${escapeHtml(edge.title)}</strong><span class="state-pill ${edge.eligible ? 'eligible' : 'blocked'}">${edge.eligible ? 'Available' : 'Blocked'}</span></div><p>→ ${escapeHtml(edge.to)}${edge.description ? ' · ' + escapeHtml(edge.description) : ''}</p>${edge.condition ? `<code>${escapeHtml(edge.condition)}</code>` : ''}${edge.blocked_reason ? `<p class="blocked-reason">${escapeHtml(edge.blocked_reason)}</p>` : ''}</div>`).join('') : '<p>No outgoing transitions.</p>'}
        <h4>Required evidence on entry</h4><p>${escapeHtml([...(node.guards || []), ...(node.action_guards || [])].join(', ') || 'No entry guard.')}</p>
        ${node.action_guard_triggers?.length ? `<h4>Capability activation triggers</h4><p>${escapeHtml(node.action_guard_triggers.join(', '))}</p>` : ''}
        ${node.user_reply_guards?.length ? `<h4>New user input required</h4><p>A new message must arrive at ${escapeHtml(node.user_reply_guards.join(' or '))}, with no subsequent transition.</p>` : ''}
        <details><summary>State settings and template</summary><pre>${escapeHtml(JSON.stringify(node.variables, null, 2))}</pre></details>`;
    if (inspector.graphMarkup !== markup) { inspector.innerHTML = markup; inspector.graphMarkup = markup; }
}

function applyGraphBox() { document.getElementById('workflow-graph').setAttribute('viewBox', `${graphBox.x} ${graphBox.y} ${graphBox.width} ${graphBox.height}`); }
function fitWorkflowGraph() { graphBox = { x: 0, y: 0, width: graphExtent.width, height: graphExtent.height }; applyGraphBox(); }
function zoomWorkflowGraph(factor, point = null) {
    const width = graphBox.width * factor, height = graphBox.height * factor;
    if (width < 180 || width > graphExtent.width * 6) return;
    const p = point || { x: graphBox.x + graphBox.width / 2, y: graphBox.y + graphBox.height / 2 };
    graphBox = { x: p.x - (p.x - graphBox.x) * factor, y: p.y - (p.y - graphBox.y) * factor, width, height };
    applyGraphBox();
}

function diffPromptLines(before, after) {
    const a = before.split('\n'), b = after.split('\n');
    let prefix = 0, suffix = 0;
    while (prefix < a.length && prefix < b.length && a[prefix] === b[prefix]) prefix++;
    while (suffix < a.length - prefix && suffix < b.length - prefix && a[a.length - 1 - suffix] === b[b.length - 1 - suffix]) suffix++;
    return { removed: a.slice(prefix, a.length - suffix), added: b.slice(prefix, b.length - suffix) };
}

async function loadExecutionTimeline() {
    const user = document.getElementById('message-user-id').value;
    if (!user) return;
    const generation = ++executionLoadGeneration;
    const list = document.getElementById('messages-list');
    list.textContent = 'Loading execution history…';
    try {
        const responses = await Promise.all([apiGet(`/api/messages/${encodeURIComponent(user)}`), apiGet(`/api/execution/${encodeURIComponent(user)}`)]);
        if (responses.some(response => !response.ok)) throw new Error('Execution history unavailable');
        const [conversation, audit] = await Promise.all(responses.map(response => response.json()));
        if (generation !== executionLoadGeneration || user !== document.getElementById('message-user-id').value) return;
        executionTimeline = { user, conversation, audit };
        renderExecutionTimeline();
    } catch (error) { if (generation === executionLoadGeneration) list.textContent = error.message; }
}

function renderExecutionTimeline() {
    if (!executionTimeline) return;
    const { conversation, audit } = executionTimeline;
    const filter = document.getElementById('message-filter').value;
    const prompts = new Map();
    for (const event of audit.events) if (event.system_prompts) prompts.set(event.prompt_hash, event.system_prompts);
    let previousPrompt = null, previousSettings = null;
    const entries = [];
    const lazyPrompts = new Map();
    for (const event of audit.events) {
        const p = event.payload;
        let html;
        if (event.kind === 'model_call') {
            const system = prompts.get(event.prompt_hash) || [];
            const body = system.join('\n\n[Next system message]\n\n');
            const change = previousPrompt == null ? null : diffPromptLines(previousPrompt, body);
            const settings = p.settings || {};
            const changedKeys = previousSettings ? [...new Set([...Object.keys(previousSettings), ...Object.keys(settings)])].filter(key => JSON.stringify(previousSettings[key]) !== JSON.stringify(settings[key])) : [];
            const changes = changedKeys.map(key => `${key}: ${JSON.stringify(previousSettings[key])} → ${JSON.stringify(settings[key])}`).join('\n');
            previousPrompt = body; previousSettings = settings;
            lazyPrompts.set(String(event.id), { body, change, changes });
            if (filter === 'changes' || filter === 'conversation') continue;
            const usage = p.usage;
            html = `<article class="audit-card"><header><span class="audit-kind">MODEL CALL ${event.id}</span><span class="state-pill ${p.status === 'completed' ? 'eligible' : 'blocked'}">${escapeHtml(p.status)}</span><time>${escapeHtml(event.created_at)}</time></header>
                <h3>${escapeHtml(p.provider)} / ${escapeHtml(p.model || 'provider default')}</h3><p>${escapeHtml(p.state || 'unset')} · ${escapeHtml(p.template || 'default template')} · attempt ${p.attempt}</p>
                <div class="audit-metrics"><span>${usage ? `${formatCount(usage.prompt_tokens)} in / ${formatCount(usage.completion_tokens)} out` : 'Usage unavailable'}</span><span>${p.tokens_per_sec == null ? 'Speed unavailable' : Number(p.tokens_per_sec).toFixed(1) + ' tokens/s'}</span><span>Output limit ${formatCount(p.output_limit)}</span><span>History ≈ ${formatCount(p.limits?.history_tokens_estimate)} / ${formatCount(p.limits?.history_limit)}</span><span>${p.limits?.compaction_enabled ? 'Compaction at ≈ ' + formatCount(p.limits.compaction_threshold) : 'Compaction off'}</span></div>
                <details class="audit-prompt" data-audit-id="${event.id}"><summary>Actual system prompts · ${system.length} message${system.length === 1 ? '' : 's'}${change ? (change.added.length || change.removed.length ? ' · changed' : ' · unchanged') : ' · first capture'}</summary></details>
                <details class="audit-changes" data-audit-id="${event.id}"><summary>Prompt and context changes${changedKeys.length ? ' · ' + changedKeys.length + ' settings' : ''}</summary></details>
                <details><summary>Tools, IR and request details</summary><pre>${escapeHtml(JSON.stringify({ decision_ir: p.decision_ir, tools: p.tools, limits: p.limits, elapsed_ms: p.elapsed_ms, first_output_ms: p.first_token_ms }, null, 2))}</pre></details></article>`;
        } else {
            if (filter === 'model' || filter === 'conversation') continue;
            html = `<article class="audit-card audit-event"><header><span class="audit-kind">${escapeHtml(event.kind.replaceAll('_', ' '))}</span><time>${escapeHtml(event.created_at)}</time></header><h3>${event.kind === 'state_transition' ? `${escapeHtml(p.from_state)} → ${escapeHtml(p.to_state)}${p.back ? ' · back' : ''}` : 'Compaction · ' + escapeHtml(p.status)}</h3><details><summary>Execution evidence</summary><pre>${escapeHtml(JSON.stringify(p, null, 2))}</pre></details></article>`;
        }
        entries.push({ cursor: event.message_cursor, order: 1, id: event.id, html });
    }
    if (filter === 'all' || filter === 'conversation') for (const message of conversation.messages || []) {
        entries.push({ cursor: message.id, order: 0, id: message.id, html: `<article class="audit-card conversation-entry"><header><span class="audit-kind">${escapeHtml(message.role)}${message.tool_name ? ' · ' + escapeHtml(message.tool_name) : ''}</span><span>#${message.id}</span></header>${message.tool_calls?.length ? `<pre>${escapeHtml(JSON.stringify(message.tool_calls, null, 2))}</pre>` : ''}<pre>${escapeHtml(message.content || '')}</pre>${message.total_tokens != null ? `<p>${formatCount(message.prompt_tokens)} in · ${formatCount(message.completion_tokens)} out</p>` : ''}</article>` });
    }
    entries.sort((a, b) => a.cursor - b.cursor || a.order - b.order || a.id - b.id);
    const limits = audit.limits || {};
    const summary = `<div class="timeline-summary"><strong>${conversation.message_count} messages · ${audit.events.length} audit events</strong><span>History ≈ ${formatCount(limits.history_tokens_estimate)} / ${formatCount(limits.history_limit)} · ${limits.compaction_enabled ? 'compaction at ≈ ' + formatCount(limits.compaction_threshold) : 'compaction off'}</span><small>Audit starts with this version. Earlier system prompts were not captured. Context-window size is unavailable; history and output limits are shown separately.</small></div>`;
    const list = document.getElementById('messages-list');
    list.innerHTML = summary + (entries.map(entry => entry.html).join('') || '<p class="empty-state">No matching entries.</p>');
    for (const details of list.querySelectorAll('[data-audit-id]')) details.addEventListener('toggle', () => {
        if (!details.open || details.dataset.loaded) return;
        const prompt = lazyPrompts.get(details.dataset.auditId);
        const pre = document.createElement('pre');
        if (details.classList.contains('audit-prompt')) pre.textContent = prompt.body || 'No system message in this request.';
        else {
            const delta = prompt.change;
            pre.textContent = (delta ? [...delta.removed.map(line => '- ' + line), ...delta.added.map(line => '+ ' + line)].join('\n') || 'System prompt unchanged.' : 'First captured system prompt; no earlier snapshot.') + '\n\n' + (prompt.changes || 'No captured context-setting changes.');
        }
        details.append(pre); details.dataset.loaded = 'true';
    });
}

document.addEventListener('DOMContentLoaded', () => {
    for (const id of ['graph-refresh', 'graph-user-id', 'graph-workflow']) document.getElementById(id).addEventListener(id === 'graph-refresh' ? 'click' : 'change', loadGraphs);
    document.getElementById('graph-fit').addEventListener('click', fitWorkflowGraph);
    document.getElementById('graph-zoom-in').addEventListener('click', () => zoomWorkflowGraph(0.8));
    document.getElementById('graph-zoom-out').addEventListener('click', () => zoomWorkflowGraph(1.25));
    document.getElementById('message-filter').addEventListener('change', renderExecutionTimeline);
    document.getElementById('message-user-id').addEventListener('change', () => { executionLoadGeneration++; executionTimeline = null; document.getElementById('messages-list').replaceChildren(); });
    const svg = document.getElementById('workflow-graph');
    svg.addEventListener('click', event => { const node = event.target.closest('[data-node]'); if (!graphMoved && (node || graphPressedNode)) inspectGraphNode(node?.dataset.node || graphPressedNode); graphPressedNode = null; });
    svg.addEventListener('keydown', event => { if (event.key === 'Enter' || event.key === ' ') { const node = event.target.closest('[data-node]'); if (node) { event.preventDefault(); inspectGraphNode(node.dataset.node); } } });
    svg.addEventListener('wheel', event => { event.preventDefault(); const matrix = svg.getScreenCTM(); if (matrix) zoomWorkflowGraph(event.deltaY > 0 ? 1.1 : 0.9, new DOMPoint(event.clientX, event.clientY).matrixTransform(matrix.inverse())); }, { passive: false });
    let drag = null;
    svg.addEventListener('pointerdown', event => { if (event.button !== 0) return; const matrix = svg.getScreenCTM(); if (!matrix) return; graphMoved = false; graphPressedNode = event.target.closest('[data-node]')?.dataset.node || null; drag = { id:event.pointerId, x:event.clientX, y:event.clientY, inverse:matrix.inverse(), box:{...graphBox} }; svg.setPointerCapture(event.pointerId); });
    svg.addEventListener('pointermove', event => { if (!drag || drag.id !== event.pointerId) return; if (Math.hypot(event.clientX - drag.x, event.clientY - drag.y) > 4) graphMoved = true; if (!graphMoved) return; const a = new DOMPoint(drag.x,drag.y).matrixTransform(drag.inverse), b = new DOMPoint(event.clientX,event.clientY).matrixTransform(drag.inverse); graphBox = {...drag.box, x:drag.box.x + a.x - b.x, y:drag.box.y + a.y - b.y}; applyGraphBox(); });
    for (const name of ['pointerup','pointercancel']) svg.addEventListener(name, event => { if (drag?.id === event.pointerId) { if (svg.hasPointerCapture(event.pointerId)) svg.releasePointerCapture(event.pointerId); drag = null; } });
    setInterval(() => { if (authToken && document.getElementById('dashboard-screen').classList.contains('active') && document.getElementById('tab-graphs').classList.contains('active') && !document.hidden) loadGraphs(); }, 5000);
});
