#!/usr/bin/env node
// Actual dashboard + SSE + stored tool calls. Commands are inert fixture text.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
let chromium;
for (const mod of ['playwright', '/tmp/praxis-ui-browser/node_modules/playwright']) {
    try { ({ chromium } = require(mod)); break; } catch (_) {}
}
assert(chromium, 'Playwright required');
const root = process.env.PRAXIS_STATIC_DIR || path.resolve(__dirname, '../static');
const longCommand = "python3 <<'PY'\n" + Array.from({ length: 90 }, (_, i) => `print(${i}, '日本語 & <script>window.injected = true</script>')`).join('\n') + '\nPY\nprintf "FULL_COMMAND_END"';
const call = (id, name, command) => ({ id, name, arguments: JSON.stringify({ command }) });
const message = (id, calls) => ({ id, role: 'assistant', content: '', tool_calls: calls });
const messages = { default: [message(10, [call('history-call', 'execute_terminal', longCommand), { id: 'file', name: 'write_file', arguments: JSON.stringify({ path: '/fixture', content: 'PRIVATE_FILE_BODY_NOT_A_COMMAND' }) }])], other: [] };
const streams = new Set();
let delayNextHistory = false;
let delayedHistory;
let clearMarker = 0;
function json(res, data, status = 200) { res.writeHead(status, { 'content-type': 'application/json' }); res.end(JSON.stringify(data)); }
const server = http.createServer((req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1');
    const p = url.pathname;
    if (p === '/' || p.startsWith('/static/') || p.startsWith('/api/avatar/') || ['/logo.svg', '/logo.png'].includes(p)) {
        const name = p === '/' ? 'index.html' : p.startsWith('/api/avatar/') ? 'logo.png' : path.basename(p);
        res.writeHead(200, { 'content-type': { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.png': 'image/png', '.svg': 'image/svg+xml' }[path.extname(name)] || 'application/octet-stream' });
        res.end(fs.readFileSync(path.join(root, name))); return;
    }
    if (p === '/api/status') return json(res, { version: 'terminal-fixture' });
    if (p === '/api/contexts') return json(res, { contexts: [{ user_id: 'default' }, { user_id: 'other' }] });
    if (p === '/api/pairings') return json(res, { pairings: [] });
    if (p === '/api/pairings/pending') return json(res, { pending_pairings: [] });
    let m;
    if ((m = p.match(/^\/api\/contexts\/(\w+)$/))) return json(res, { user_id: m[1], username: 'Fixture', settings: { web_chat_tts: false } });
    if (p === '/api/messages/default/clear-chat' && req.method === 'POST') {
        clearMarker = Math.max(...messages.default.map(m => m.id));
        return json(res, { success: true });
    }
    if ((m = p.match(/^\/api\/messages\/(\w+)$/))) {
        assert.equal(req.headers.authorization, 'Bearer synthetic-terminal-token');
        const data = { messages: messages[m[1]].filter(row => m[1] !== 'default' || row.id > clearMarker) };
        if (m[1] === 'default' && delayNextHistory) { delayNextHistory = false; delayedHistory = { res, data }; return; }
        return json(res, data);
    }
    if (/^\/api\/agent\/status\//.test(p)) return json(res, { active: false });
    if (/^\/api\/(sm|cl)\//.test(p)) return json(res, {});
    if ((m = p.match(/^\/api\/chat\/stream\/(\w+)$/))) {
        res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' }); res.write(': ready\n\n');
        const s = { uid: m[1], res }; streams.add(s); req.on('close', () => streams.delete(s)); return;
    }
    json(res, {}, 404);
});
function emit(payload) {
    for (const s of streams) if (s.uid === 'default') s.res.write(`event: tool_call\ndata: ${JSON.stringify({ event: 'tool_call', data: JSON.stringify(payload) })}\n\n`);
}
async function commandShown(page, command) {
    await page.waitForFunction(text => [...document.querySelectorAll('.terminal-command')].some(p => p.textContent === text), command, { timeout: 10000 });
}
(async () => {
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
    const base = `http://127.0.0.1:${server.address().port}`;
    let browser;
    try {
        browser = await chromium.launch({ headless: true });
        const page = await browser.newPage({ viewport: { width: 390, height: 820 } });
        const errors = []; page.on('pageerror', e => errors.push(e.message));
        await page.route('**/*', route => new URL(route.request().url()).origin === base ? route.continue() : route.abort());
        await page.addInitScript(() => {
            localStorage.setItem('praxis_token', 'synthetic-terminal-token');
            localStorage.setItem('praxis_chat_sessions', JSON.stringify([{ id: 'default', name: 'Default' }, { id: 'other', name: 'Other' }]));
            Object.defineProperty(navigator, 'clipboard', { value: { writeText: async text => { window.copiedCommand = text; } }, configurable: true });
        });
        await page.goto(base, { waitUntil: 'domcontentloaded' });
        await page.evaluate(() => showTab('chat'));
        await commandShown(page, longCommand);
        assert.equal(await page.locator('.chat-terminal-call').count(), 1);
        assert.equal(await page.evaluate(() => window.injected), undefined, 'command text must never execute as HTML');
        assert(!(await page.locator('#chat-messages').textContent()).includes('PRIVATE_FILE_BODY_NOT_A_COMMAND'));
        assert.equal(await page.locator('.terminal-command').evaluate(el => getComputedStyle(el).maxHeight), 'none');
        assert(await page.locator('.terminal-command').evaluate(el => el.scrollHeight <= el.clientHeight + 1), 'full command must not be clipped');
        assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), 'long commands must wrap on mobile');
        const copy = page.locator('.terminal-command-copy');
        assert(await copy.evaluate(el => el.classList.contains('btn-secondary')), 'copy button should use dashboard styling');
        assert(await copy.evaluate(el => parseFloat(getComputedStyle(el).borderRadius) >= 6));
        await copy.focus();
        assert.equal(await copy.evaluate(el => getComputedStyle(el).outlineStyle), 'solid', 'keyboard focus must be visible');
        await copy.click();
        assert.equal(await page.evaluate(() => window.copiedCommand), longCommand);
        for (const theme of ['dark', 'light']) {
            await page.evaluate(theme => document.documentElement.setAttribute('data-theme', theme), theme);
            assert(await copy.evaluate(el => {
                const swatch = document.createElement('span');
                swatch.style.color = 'var(--text-primary)'; el.append(swatch);
                const matches = getComputedStyle(el).color === getComputedStyle(swatch).color;
                swatch.remove(); return matches;
            }), `copy text should follow ${theme} theme`);
        }
        await page.evaluate(() => document.documentElement.setAttribute('data-theme', 'dark'));

        // Old servers send only 200 characters of JSON; load the saved original immediately.
        const live = longCommand + '\nprintf "LIVE_END"';
        messages.default.push(message(11, [call('live-call', 'run_background', live)]));
        const event = { tool: 'run_background', call_id: 'live-call', args_preview: { args: JSON.stringify({ command: live }).slice(0, 200) } };
        emit(event);
        await commandShown(page, live);
        emit(event);
        await page.evaluate(() => pollChatMessages());
        assert.equal(await page.locator('.chat-terminal-call').count(), 2, 'SSE and history must deduplicate');
        await page.reload({ waitUntil: 'domcontentloaded' });
        await commandShown(page, live);
        assert.equal(await page.locator('.chat-terminal-call').count(), 2, 'commands survive reload');

        // Provider call IDs may be reused in different assistant messages.
        const reused = 'printf "SECOND_MESSAGE_SAME_CALL_ID"';
        messages.default.push(message(12, [call('live-call', 'vm_shell', reused)]));
        await page.evaluate(() => pollChatMessages());
        await commandShown(page, reused);
        assert.equal(await page.locator('.chat-terminal-call').count(), 3);

        // /clear must invalidate old recovery, without blocking newer commands.
        await page.evaluate(() => stopChatPolling());
        messages.default.push(message(13, [call('clear-call', 'execute_terminal', 'printf "CLEARED_COMMAND"')]));
        delayNextHistory = true;
        emit({ tool: 'execute_terminal', call_id: 'clear-call', args_preview: { args: '{"command":' } });
        await page.waitForFunction(() => document.querySelectorAll('.chat-terminal-call').length === 4);
        await new Promise((resolve, reject) => {
            const started = Date.now();
            const timer = setInterval(() => {
                if (delayedHistory) { clearInterval(timer); resolve(); }
                else if (Date.now() - started > 10000) { clearInterval(timer); reject(new Error('clear recovery not requested')); }
            }, 20);
        });
        await page.evaluate(() => clearChatMessages());
        const afterClear = 'printf "NEW_AFTER_CLEAR"';
        messages.default.push(message(14, [call('after-clear', 'execute_terminal', afterClear)]));
        emit({ tool: 'execute_terminal', call_id: 'after-clear', args_preview: { args: '{"command":' } });
        await commandShown(page, afterClear);
        json(delayedHistory.res, delayedHistory.data); delayedHistory = null;
        await page.evaluate(() => pollChatMessages());
        assert.equal(await page.locator('.chat-terminal-call').count(), 1, 'cleared commands must stay hidden');

        // A delayed command recovery and a queued event must not cross sessions.
        await page.evaluate(() => { stopChatPolling(); window.previousStream = chatEventSource; });
        const late = 'printf "LATE_PRIVATE_COMMAND"';
        messages.default.push(message(15, [call('late-call', 'execute_terminal', late)]));
        delayNextHistory = true;
        emit({ tool: 'execute_terminal', call_id: 'late-call', args_preview: { args: '{"command":' } });
        await page.waitForFunction(() => document.querySelectorAll('.chat-terminal-call').length === 2);
        await new Promise((resolve, reject) => {
            const started = Date.now();
            const timer = setInterval(() => {
                if (delayedHistory) { clearInterval(timer); resolve(); }
                else if (Date.now() - started > 10000) { clearInterval(timer); reject(new Error('history recovery not requested')); }
            }, 20);
        });
        await page.evaluate(() => switchChatSession('other'));
        json(delayedHistory.res, delayedHistory.data); delayedHistory = null;
        await page.evaluate(() => window.previousStream.dispatchEvent(new MessageEvent('tool_call', { data: JSON.stringify({ data: JSON.stringify({ tool: 'execute_terminal', call_id: 'old-source', args_preview: { command: 'STALE_EVENT_COMMAND' } }) }) })));
        await page.evaluate(() => pollChatMessages());
        assert.equal(await page.locator('.chat-terminal-call').count(), 0, 'late command must not enter another session');
        assert(!(await page.locator('#chat-messages').textContent()).includes('LATE_PRIVATE_COMMAND'));
        assert.deepEqual(errors, []);
        console.log('test_chat_terminal: ok (full commands, SSE recovery, history, copy, Unicode/XSS, wrapping, dedup, clear/session isolation)');
    } finally {
        if (browser) await browser.close();
        for (const s of streams) s.res.end();
        if (delayedHistory) delayedHistory.res.end();
        server.closeAllConnections();
        await new Promise(resolve => server.close(resolve));
    }
})().catch(e => { console.error(e); process.exitCode = 1; });
