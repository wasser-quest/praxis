#!/usr/bin/env node
// Real dashboard with synthetic stored messages and controllable SSE/HTTP races.
// No live service, private history, credentials or provider calls.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const http = require('node:http');
let chromium;
for (const mod of ['playwright', '/tmp/praxis-ui-browser/node_modules/playwright']) {
    try { ({ chromium } = require(mod)); break; } catch (_) {}
}
assert(chromium, 'Playwright required');
const root = process.env.PRAXIS_STATIC_DIR || path.resolve(__dirname, '../static');
const streams = new Set();
let messages, delayNext, delayed, failNext, audioRequests;
const assistant = (id, content, tool_calls) => ({ id, role: 'assistant', content, ...(tool_calls ? { tool_calls } : {}) });
const complete = [{ id: 'complete', name: 'agent_complete', arguments: '{}' }];
function json(res, data, status = 200) { res.writeHead(status, { 'content-type': 'application/json' }); res.end(JSON.stringify(data)); }
const server = http.createServer((req, res) => {
    const p = new URL(req.url, 'http://127.0.0.1').pathname;
    if (p === '/' || p.startsWith('/static/') || p.startsWith('/api/avatar/') || ['/logo.svg', '/logo.png'].includes(p)) {
        const name = p === '/' ? 'index.html' : p.startsWith('/api/avatar/') ? 'logo.png' : path.basename(p);
        res.writeHead(200, { 'content-type': { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.png': 'image/png', '.svg': 'image/svg+xml' }[path.extname(name)] || 'application/octet-stream' });
        return res.end(fs.readFileSync(path.join(root, name)));
    }
    if (p === '/api/status') return json(res, { version: 'history-fixture' });
    if (p === '/api/contexts') return json(res, { contexts: [{ user_id: 'default' }, { user_id: 'other' }] });
    if (p === '/api/pairings') return json(res, { pairings: [] });
    if (p === '/api/pairings/pending') return json(res, { pending_pairings: [] });
    let m;
    if ((m = p.match(/^\/api\/contexts\/(\w+)$/))) return json(res, { user_id: m[1], username: 'Fixture', settings: { web_chat_tts: true } });
    if ((m = p.match(/^\/api\/messages\/(\w+)$/))) {
        assert.equal(req.headers.authorization, 'Bearer synthetic-history-token');
        if (failNext) { failNext = false; return json(res, { error: 'synthetic unavailable' }, 503); }
        const data = JSON.parse(JSON.stringify({ messages: messages[m[1]] || [] }));
        if (m[1] === 'default' && delayNext) { delayNext = false; delayed = { res, data }; return; }
        return json(res, data);
    }
    if (/^\/api\/chat\/audio\//.test(p)) { audioRequests++; return json(res, {}, 404); }
    if (/^\/api\/agent\/status\//.test(p)) return json(res, { active: false });
    if (/^\/api\/(sm|cl)\//.test(p)) return json(res, {});
    if ((m = p.match(/^\/api\/chat\/stream\/(\w+)$/))) {
        res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' }); res.write(': ready\n\n');
        const s = { uid: m[1], res }; streams.add(s); req.on('close', () => streams.delete(s)); return;
    }
    json(res, {}, 404);
});
function emit(event, payload) {
    for (const s of streams) if (s.uid === 'default') {
        const data = typeof payload === 'string' ? payload : JSON.stringify(payload);
        s.res.write(`event: ${event}\ndata: ${JSON.stringify({ event, data })}\n\n`);
    }
}
async function until(predicate) {
    const start = Date.now();
    while (!predicate()) {
        if (Date.now() - start > 10000) throw new Error('fixture wait timed out');
        await new Promise(resolve => setTimeout(resolve, 10));
    }
}
const row = (page, id) => page.locator(`#chat-messages .chat-msg[data-message-id="${id}"]`);
async function visible(page, id) { await row(page, id).waitFor({ state: 'attached', timeout: 8000 }); }
function releaseHistory() { const d = delayed; delayed = null; json(d.res, d.data); }
(async () => {
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
    const base = `http://127.0.0.1:${server.address().port}`;
    const browser = await chromium.launch({ headless: true });
    const failures = [];
    async function check(name, seed, run) {
        if (process.env.CASE && process.env.CASE !== name) return;
        messages = { default: seed, other: [] }; delayNext = false; delayed = null; failNext = false; audioRequests = 0;
        const page = await browser.newPage({ viewport: { width: 980, height: 820 } });
        const errors = []; page.on('pageerror', e => errors.push(e.message));
        try {
            await page.route('**/*', route => new URL(route.request().url()).origin === base ? route.continue() : route.abort());
            await page.addInitScript(() => {
                localStorage.setItem('praxis_token', 'synthetic-history-token');
                localStorage.setItem('praxis_chat_sessions', JSON.stringify([{ id: 'default', name: 'Default' }, { id: 'other', name: 'Other' }]));
            });
            await page.goto(base, { waitUntil: 'domcontentloaded' });
            await page.waitForFunction(() => chatEventSource?.readyState === EventSource.OPEN && chatTtsSettings.has('default'));
            await page.evaluate(async () => { showTab('chat'); await pollChatMessages(); stopChatPolling(); });
            await run(page);
            assert.deepEqual(errors, []);
            console.log('PASS', name);
        } catch (e) { failures.push(name); console.error('FAIL', name, e.message); }
        finally {
            if (delayed) { delayed.res.end(); delayed = null; }
            await page.close();
        }
    }
    try {

        const structuredRaw = JSON.stringify({stdout:'FIRST\nSECOND 世界',stderr:'warning\nlast',exit_code:3});
        await check('structured-results', [
            {id:41,role:'tool',tool_name:'execute_terminal',tool_call_id:'structured',content:structuredRaw}
        ], async page => {
            const output = row(page,41).locator('.tool-out').first();
            assert((await output.textContent()).includes('FIRST\nSECOND 世界'), 'stdout must decode JSON newlines');
            assert(!(await output.textContent()).includes('\\n'), 'presentation must not show serialized escapes');
        });

        await check('structured-results-corpus', [], async page => {
            const fixtures = JSON.parse(fs.readFileSync(path.join(__dirname,'fixtures/structured_tool_results.json'),'utf8'));
            for (const fixture of fixtures) {
                const actual = await page.evaluate(f => chatFormatToolResult(f.raw), fixture);
                assert.equal(actual, fixture.display, fixture.name);
            }
            const large = '世界 👩‍💻\n  next\n'.repeat(10000);
            const raw = JSON.stringify({stdout:large,stderr:'',exit_code:0});
            assert((await page.evaluate(raw => chatFormatToolResult(raw),raw)).includes(large));
            // Use the real renderer, not innerHTML, even for script-shaped output.
            const script = '<script>window.injected = true</script>\n<img src=x onerror="window.injected = true">';
            messages.default = [{id:71,role:'tool',tool_name:'nonterminal_fixture',content:JSON.stringify({stdout:script,exit_code:0})},
                {id:72,role:'tool',tool_name:'empty_fixture',content:''}];
            await page.evaluate(() => pollChatMessages());
            assert((await row(page,71).locator('.tool-out').textContent()).includes(script));
            assert.equal(await row(page,71).locator('.tool-out script, .tool-out img').count(),0);
            assert.equal(await page.evaluate(() => window.injected), undefined);
            assert.equal(await row(page,72).locator('.tool-out').textContent(),'');
            assert.equal(await row(page,72).locator('.tool-result-raw').isVisible(),false);
        });
        await check('structured-results-controls', [
            {id:41,role:'tool',tool_name:'execute_terminal',tool_call_id:'structured',content:structuredRaw}
        ], async page => {
            await page.evaluate(() => Object.defineProperty(navigator,'clipboard',{configurable:true,
                value:{writeText:async text=> {window.fixtureCopy=text;}}}));
            const output = row(page,41).locator('.tool-out');
            const toggle = row(page,41).locator('.tool-result-raw');
            await toggle.focus();
            assert.equal(await toggle.evaluate(el=>getComputedStyle(el).outlineStyle),'solid');
            await toggle.press('Enter');
            assert.equal(await output.textContent(),structuredRaw);
            assert.equal(await toggle.getAttribute('aria-pressed'),'true');
            await row(page,41).locator('.tool-result-copy').click();
            assert.equal(await page.evaluate(()=>window.fixtureCopy),structuredRaw);
            // Polling must leave the exact text node, raw mode and selection intact.
            await output.evaluate(el=>{window.originalResultNode=el.firstChild;const range=document.createRange();
                range.selectNodeContents(el);getSelection().removeAllRanges();getSelection().addRange(range);});
            await page.evaluate(async()=>{await pollChatMessages();await pollChatMessages();});
            assert(await output.evaluate(el=>el.firstChild===window.originalResultNode));
            assert.equal(await page.evaluate(()=>getSelection().toString()),structuredRaw);
            assert.equal(await toggle.getAttribute('aria-pressed'),'true');
            assert.equal(messages.default[0].content,structuredRaw);
            await toggle.click();
            assert((await output.textContent()).includes('FIRST\nSECOND 世界'));
            const changed = JSON.stringify({stdout:'UPDATED\nLAST',stderr:'',exit_code:0});
            messages.default[0].content=changed;
            await page.evaluate(()=>pollChatMessages());
            await toggle.click();
            assert.equal(await output.textContent(),changed,'toggle closure must use updated receipt');
            await row(page,41).locator('.tool-result-copy').click();
            assert.equal(await page.evaluate(()=>window.fixtureCopy),changed);
            await page.reload({waitUntil:'domcontentloaded'});
            await page.evaluate(async()=>{showTab('chat');await pollChatMessages();stopChatPolling();});
            assert((await row(page,41).locator('.tool-out').textContent()).includes('UPDATED\nLAST'));
            await page.evaluate(()=>switchChatSession('other'));
            assert.equal(await page.locator('.tool-result-controls').count(),0);
        });
        await check('structured-results-live-history', [], async page => {
            const full=JSON.stringify({stdout:'live\n'+'世界 '.repeat(1000)+'\nFULL_END',stderr:'',exit_code:0});
            const preview=full.slice(0,80);
            emit('tool_result',{tool:'execute_terminal',call_id:'receipt',duration_ms:2,result:preview});
            await page.waitForFunction(()=>document.querySelector('.tool-result-note')?.textContent.includes('Live-Vorschau'));
            const provisional=page.locator('[data-tool-result-call-id="receipt"]');
            assert.equal(await provisional.locator('.tool-out').textContent(),preview,'incomplete JSON must not be guessed');
            assert.equal(await provisional.locator('.tool-result-copy').textContent(),'Vorschau kopieren');
            messages.default=[{id:81,role:'tool',tool_name:'execute_terminal',tool_call_id:'receipt',content:full}];
            await page.evaluate(async()=>{await pollChatMessages();await pollChatMessages();});
            assert.equal(await page.locator('#chat-messages .chat-msg.tool').count(),1);
            assert((await row(page,81).locator('.tool-out').textContent()).includes('\nFULL_END'));
            assert.equal(await row(page,81).locator('.tool-result-note').textContent(),'');
            messages.default.push({...messages.default[0],id:82});
            await page.evaluate(()=>pollChatMessages());
            assert.equal(await page.locator('#chat-messages .chat-msg.tool').count(),2,'reused call IDs in different saved rows remain distinct');
        });

        await check('structured-results-unchanged-poll', [], async page => {
            messages.default = Array.from({length:200},(_,i)=>({id:i+1,role:'tool',tool_name:'execute_terminal',
                content:JSON.stringify({stdout:'LONG 世界\n'.repeat(100),stderr:'',exit_code:0})}));
            await page.evaluate(()=>pollChatMessages());
            await page.evaluate(()=>{
                window.originalFormatter=chatFormatToolResult;window.resultFormatCalls=0;
                chatFormatToolResult=(...args)=>{window.resultFormatCalls++;return window.originalFormatter(...args);};
            });
            const output = row(page,1).locator('.tool-out');
            await output.evaluate(el=>{el.scrollTop=120;window.resultScroll=el.scrollTop;});
            await page.evaluate(async()=>{await pollChatMessages();await pollChatMessages();});
            assert.equal(await page.evaluate(()=>window.resultFormatCalls),0,'unchanged receipts must not be reparsed on polling');
            assert(await output.evaluate(el=>el.scrollTop===window.resultScroll),'unchanged poll must preserve output scroll');
            for(const width of [360,980]) {
                await page.setViewportSize({width,height:820});
                assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),'controls/output must fit mobile');
            }
            // A command that looks like a receipt is still an inert exact command.
            const command=JSON.stringify({stdout:'NOT\nA_RESULT'});
            await page.evaluate(command=>renderChatMessage({id:500,role:'assistant',content:'',tool_calls:[
                {id:'json-command',name:'execute_terminal',arguments:JSON.stringify({command})}]}),command);
            assert.equal(await page.locator('.terminal-command').textContent(),command);
        });
        const prefix = 'Identical prefix '.repeat(8);
        await check('stored-responses', [
            assistant(1, 'Complete answer alongside a tool call.', complete),
            assistant(2, prefix + 'FIRST'), assistant(3, prefix + 'SECOND'),
            assistant(4, 'Repeated answer'), assistant(5, 'Repeated answer'),
            { id: 6, role: 'user', content: prefix + 'user one' }, { id: 7, role: 'user', content: prefix + 'user two' },
            { id: 8, role: 'tool', content: 'same result', tool_name: 'execute_terminal', tool_call_id: 'one' },
            { id: 9, role: 'tool', content: 'same result', tool_name: 'execute_terminal', tool_call_id: 'two' },
            assistant(10, '', [{ id: 'command', name: 'execute_terminal', arguments: '{"command":"printf fixture"}' }]),
        ], async page => {
            await visible(page, 1);
            for (let id = 1; id <= 9; id++) { await visible(page, id); assert.equal(await row(page, id).count(), 1); }
            assert.equal(await page.locator('.chat-msg.assistant').count(), 5);
            assert.equal(await page.locator('.chat-terminal-call').count(), 1);
            await page.evaluate(async () => { await pollChatMessages(); await loadChatHistory(); await pollChatMessages(); });
            for (let id = 1; id <= 9; id++) assert.equal(await row(page, id).count(), 1, `stable identity ${id}`);
        });
        await check('return-to-chat', [], async page => {
            await page.evaluate(() => showTab('overview'));
            messages.default.push({ ...assistant(20, 'Reply received while another tab was open.', complete), audio_mime: 'audio/wav' });
            // Intentionally no SSE/reconnect: returning to chat must refresh even while idle.
            await page.evaluate(() => showTab('chat'));
            await visible(page, 20);
            assert.equal(audioRequests, 0, 'opening history must not autoplay old replies');
            assert.equal(await row(page, 20).locator('.chat-audio-play').count(), 1);
        });
        await check('reload-during-stream', [], async page => {
            emit('char', 'Live ');
            await page.waitForFunction(() => document.querySelector('.stream-live')?.textContent === 'Live ');
            delayNext = true;
            await page.evaluate(() => { window.loadingHistory = loadChatHistory(); });
            await until(() => delayed);
            assert.equal(await page.locator('.stream-live').count(), 1, 'loading must not detach the active stream');
            emit('char', 'answer'); emit('assistant', 'Live answer');
            messages.default.push(assistant(30, 'Live answer', complete));
            await page.waitForFunction(() => [...document.querySelectorAll('.assistant')].some(n => n.chatText === 'Live answer'));
            releaseHistory(); await page.evaluate(() => window.loadingHistory);
            assert.equal(await page.locator('.chat-msg.assistant').count(), 1, 'stale empty history must not erase a new response');
            await page.evaluate(() => pollChatMessages()); await visible(page, 30);
            failNext = true; await page.evaluate(() => loadChatHistory());
            assert.equal(await row(page, 30).count(), 1, 'HTTP failure must preserve the visible history');
        });
        await check('saved-during-reload', [], async page => {
            emit('char', 'Saving ');
            await page.waitForFunction(() => document.querySelector('.stream-live'));
            delayNext = true;
            await page.evaluate(() => { window.pendingReload = loadChatHistory(); }); await until(() => delayed);
            messages.default.push(assistant(31, 'Saving answer'));
            emit('char', 'answer'); emit('assistant', 'Saving answer'); emit('assistant_saved', messages.default[0]);
            await visible(page, 31);
            releaseHistory(); await page.evaluate(() => window.pendingReload);
            assert.equal(await row(page, 31).count(), 1, 'a provisional node bound during reload is still a new reply');
        });
        await check('saved-stream-races', [], async page => {
            emit('char', 'Race ');
            await page.waitForFunction(() => document.querySelector('.stream-live'));
            messages.default.push(assistant(40, 'Race reply'));
            await page.evaluate(() => pollChatMessages());
            emit('char', 'reply'); emit('assistant', 'Race reply'); emit('assistant_saved', messages.default[0]);
            await visible(page, 40);
            await page.waitForFunction(() => document.querySelectorAll('.stream-live').length === 0);
            assert.equal(await row(page, 40).count(), 1, 'poll-before-save must not duplicate a streamed answer');
            emit('assistant', 'Older unsaved reply'); emit('assistant', 'Newer unsaved reply');
            await page.waitForFunction(() => [...document.querySelectorAll('.assistant')].some(n => n.chatText === 'Newer unsaved reply'));
            emit('assistant_saved', assistant(41, 'Older unsaved reply'));
            await visible(page, 41);
            assert(await page.locator('#chat-messages').textContent().then(t => t.includes('Newer unsaved reply')), 'late save must not overwrite a different reply');
            emit('assistant_saved', assistant(42, 'Newer unsaved reply'));
            await visible(page, 42);
            for (const id of [40, 41, 42]) assert.equal(await row(page, id).count(), 1);
        });
        await check('out-of-order-reload', [assistant(60, 'Already visible')], async page => {
            await visible(page, 60);
            messages.default = []; delayNext = true;
            await page.evaluate(() => { window.firstReload = loadChatHistory(); }); await until(() => delayed);
            messages.default = [assistant(60, 'Already visible'), assistant(61, 'Newer snapshot')];
            await page.evaluate(() => loadChatHistory()); await visible(page, 61);
            releaseHistory(); await page.evaluate(() => window.firstReload);
            assert.equal(await row(page, 60).count(), 1); assert.equal(await row(page, 61).count(), 1);
            // A current successful snapshot still reconciles genuinely pruned history.
            messages.default = [assistant(61, 'Newer snapshot')];
            await page.evaluate(() => loadChatHistory());
            assert.equal(await row(page, 60).count(), 0); assert.equal(await row(page, 61).count(), 1);
        });
        await check('stale-session-events', [], async page => {
            messages.default = [assistant(50, 'Private old-session reply')]; delayNext = true;
            await page.evaluate(() => { window.oldStream = chatEventSource; window.oldLoad = loadChatHistory(); }); await until(() => delayed);
            await page.evaluate(() => switchChatSession('other'));
            releaseHistory(); await page.evaluate(() => window.oldLoad);
            await page.evaluate(() => {
                for (const [event, payload] of [['char', 'STALE'], ['assistant', 'STALE'], ['assistant_saved', { id: 50, role: 'assistant', content: 'STALE' }]]) {
                    window.oldStream.dispatchEvent(new MessageEvent(event, { data: JSON.stringify({ data: typeof payload === 'string' ? payload : JSON.stringify(payload) }) }));
                }
            });
            assert.equal(await page.locator('.chat-msg.assistant').count(), 0);
        });
        await check('history-refresh-linear', [], async page => {
            // Seed already-rendered rows: measure an unchanged poll, not cold
            // layout/avatar loading. Exercise the actual HTTP/render pipeline.
            for (const mixed of [false, true]) for (const n of [100, 500, 1000]) {
                messages.default = Array.from({ length: n }, (_, i) => ({
                    id: i + 1, role: mixed && i % 2 ? 'assistant' : 'user',
                    content: `unchanged ${i}`,
                }));
                await page.evaluate(rows => {
                    const container = document.getElementById('chat-messages');
                    container.replaceChildren();
                    for (const m of rows) {
                        const div = document.createElement('div');
                        div.className = `chat-msg ${m.role}`;
                        div.dataset.messageId = String(m.id);
                        div.chatText = m.content;
                        div.innerHTML = '<div class="msg-content"></div>';
                        div.firstChild.textContent = m.content;
                        container.appendChild(div);
                    }
                }, messages.default);
                const result = await page.evaluate(async () => {
                    let calls = 0, references = 0;
                    const restores = [];
                    for (const proto of [Document.prototype, Element.prototype]) {
                        const original = proto.querySelectorAll;
                        proto.querySelectorAll = function (...args) {
                            const nodes = original.apply(this, args);
                            calls++; references += nodes.length;
                            return nodes;
                        };
                        restores.push(() => { proto.querySelectorAll = original; });
                    }
                    const start = performance.now();
                    try { await pollChatMessages(false); }
                    finally { restores.forEach(restore => restore()); }
                    return { calls, references, ms: +(performance.now() - start).toFixed(2) };
                });
                console.log('history-refresh metrics', { mixed, n, ...result });
                assert(result.references <= 4 * n,
                    `unchanged ${mixed ? 'mixed' : 'user'} history must avoid quadratic DOM scans: ${JSON.stringify(result)}`);
                assert.equal(await page.locator('#chat-messages .chat-msg').count(), n);
                assert.equal(await row(page, n).textContent(), `unchanged ${n - 1}`);
            }
            // Tool-heavy conversations also replay saved command cards.
            messages.default = Array.from({ length: 100 }, (_, i) => assistant(2000 + i, '', [{
                id: `terminal-${i}`, name: 'execute_terminal',
                arguments: JSON.stringify({ command: `printf 'fixture-${i}'` }),
            }]));
            await page.evaluate(() => loadChatHistory());
            const terminalResult = await page.evaluate(async () => {
                let references = 0;
                const restores = [];
                for (const proto of [Document.prototype, Element.prototype]) {
                    const original = proto.querySelectorAll;
                    proto.querySelectorAll = function (...args) {
                        const nodes = original.apply(this, args);
                        references += nodes.length;
                        return nodes;
                    };
                    restores.push(() => { proto.querySelectorAll = original; });
                }
                try { await pollChatMessages(false); }
                finally { restores.forEach(restore => restore()); }
                return { references };
            });
            console.log('terminal-refresh metrics', terminalResult);
            assert(terminalResult.references <= 400,
                `unchanged terminal history must avoid quadratic scans: ${JSON.stringify(terminalResult)}`);
            assert.equal(await page.locator('.chat-terminal-call').count(), 100);
            // The per-refresh index must learn rows created during that batch,
            // while repeated text with distinct DB IDs remains distinct.
            messages.default = [assistant(1001, 'same'), assistant(1001, 'same'), assistant(1002, 'same')];
            await page.evaluate(() => loadChatHistory());
            assert.equal(await row(page, 1001).count(), 1);
            assert.equal(await row(page, 1002).count(), 1);
            // No retained index may resurrect a detached row after clear/reload.
            await page.evaluate(() => document.getElementById('chat-messages').replaceChildren());
            await page.evaluate(() => pollChatMessages(false));
            assert.equal(await row(page, 1001).count(), 1);
            assert.equal(await row(page, 1002).count(), 1);
        });
        assert.deepEqual(failures, [], 'history regressions');
        console.log('test_chat_history: ok; local fixtures only');
    } finally {
        await browser.close(); for (const s of streams) s.res.end();
        server.closeAllConnections(); await new Promise(resolve => server.close(resolve));
    }
})().catch(e => { console.error(e); process.exitCode = 1; });
