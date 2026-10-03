#!/usr/bin/env node
// Real dashboard JS + EventSource + HTMLAudioElement against local fixtures.
// Audio is synthetic silence. No live service, credentials or TTS provider.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
let chromium;
for (const mod of ['playwright', '/tmp/praxis-ui-browser/node_modules/playwright']) {
    try { ({ chromium } = require(mod)); break; } catch (_) {}
}
assert(chromium, 'Install Playwright under /tmp/praxis-ui-browser');
const root = path.resolve(__dirname, '../static');
const wav = Buffer.alloc(16044); // one second, mono, 8 kHz, 16-bit silence
wav.write('RIFF', 0); wav.writeUInt32LE(wav.length - 8, 4); wav.write('WAVEfmt ', 8);
wav.writeUInt32LE(16, 16); wav.writeUInt16LE(1, 20); wav.writeUInt16LE(1, 22);
wav.writeUInt32LE(8000, 24); wav.writeUInt32LE(16000, 28);
wav.writeUInt16LE(2, 32); wav.writeUInt16LE(16, 34);
wav.write('data', 36); wav.writeUInt32LE(16000, 40);
const messages = { default: [], other: [], discord: [] };
const ttsOn = { default: true, other: true, discord: true };
const streams = new Set();
const openings = { default: 0, other: 0, discord: 0 };
let audioRequests = 0;
let delayedAudio = null;
let delayId = null;
let failId = null;
let delayContextUid = null;
let delayedContext = null;
let failContextUid = null;
function json(res, obj, status = 200) {
    res.writeHead(status, { 'content-type': 'application/json' });
    res.end(JSON.stringify(obj));
}
const server = http.createServer((req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1');
    const p = url.pathname;
    if (p === '/' || p.startsWith('/static/') || p.startsWith('/api/avatar/') || ['/logo.svg', '/logo.png'].includes(p)) {
        const name = p === '/' ? 'index.html' : p.startsWith('/api/avatar/') ? 'logo.png' : path.basename(p);
        const type = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.png': 'image/png', '.svg': 'image/svg+xml' }[path.extname(name)];
        res.writeHead(200, { 'content-type': type || 'application/octet-stream' });
        res.end(fs.readFileSync(path.join(root, name)));
        return;
    }
    if (p === '/api/status') return json(res, { version: 'audio-fixture' });
    if (p === '/api/contexts') return json(res, { contexts: [{ user_id: 'default' }] });
    if (p === '/api/pairings') return json(res, { pairings: [{ user_id: 'discord', discord_user_id: '123' }] });
    if (p === '/api/pairings/pending') return json(res, { pending_pairings: [] });
    if (p === '/api/chat/send') return json(res, { type: 'queued' });
    let m;
    if ((m = p.match(/^\/api\/contexts\/(\w+)$/))) {
        const uid = m[1];
        if (req.method === 'PUT') {
            let data = '';
            req.on('data', c => data += c);
            req.on('end', () => {
                setTts(uid, JSON.parse(data).settings.web_chat_tts);
                json(res, { success: true });
            });
        } else {
            if (uid === failContextUid) return json(res, { error: 'fixture context failure' }, 503);
            const body = { user_id: uid, username: 'Fixture', settings: { web_chat_tts: ttsOn[uid] } };
            if (uid === delayContextUid) {
                delayContextUid = null;
                delayedContext = { res, body };
            } else json(res, body);
        }
        return;
    }
    if ((m = p.match(/^\/api\/messages\/(\w+)$/))) return json(res, { messages: messages[m[1]] || [] });
    if (/^\/api\/agent\/status\//.test(p)) return json(res, { active: false });
    if (/^\/api\/(sm|cl)\//.test(p)) return json(res, {});
    if ((m = p.match(/^\/api\/chat\/stream\/(\w+)(\/tts)?$/))) {
        res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' });
        res.write(': connected\n\n');
        const stream = { uid: m[1], side: !!m[2], res };
        streams.add(stream);
        openings[stream.uid]++;
        req.on('close', () => streams.delete(stream));
        return;
    }
    if ((m = p.match(/^\/api\/chat\/audio\/(\w+)\/(\d+)$/))) {
        assert.equal(req.headers.authorization, 'Bearer synthetic-audio-token');
        audioRequests++;
        if (Number(m[2]) === failId) return json(res, { error: 'fixture audio failure' }, 500);
        res.writeHead(200, { 'content-type': 'audio/wav' });
        if (Number(m[2]) === delayId) delayedAudio = res;
        else res.end(wav);
        return;
    }
    json(res, {}, 404);
});
function emit(uid, event, payload) {
    const data = typeof payload === 'string' ? payload : JSON.stringify(payload);
    for (const s of streams) {
        if (s.uid === uid && (!s.side || ['chat_tts', 'chat_tts_settings'].includes(event))) s.res.write(`event: ${event}\ndata: ${JSON.stringify({ event, data })}\n\n`);
    }
}
function setTts(uid, on, notify = true) {
    ttsOn[uid] = on;
    if (notify) emit(uid, 'chat_tts_settings', { enabled: on });
}
function reply(id, content) {
    const m = { id, role: 'assistant', content };
    messages.default.push(m);
    emit('default', 'agent_start', {});
    emit('default', 'char', content);
    emit('default', 'assistant', content);
    emit('default', 'assistant_saved', m);
    emit('default', 'agent_stop', {});
    return m;
}
function ready(m, uid = 'default') {
    m.audio_mime = 'audio/wav';
    emit(uid, 'chat_tts', { message_id: m.id, mime: m.audio_mime });
}
async function idle(page) { await page.waitForFunction(() => chatTtsState === 'idle'); }
async function playing(page, id) {
    await page.waitForFunction(id => chatTtsState === 'playing' && chatTtsCurrent.messageId === id, id);
}
function button(page, id) { return page.locator(`.assistant[data-message-id="${id}"] .chat-audio-play`); }

(async () => {
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
    const base = `http://127.0.0.1:${server.address().port}`;
    let browser;
    try {
        browser = await chromium.launch({ headless: true });
        const page = await browser.newPage({ viewport: { width: 390, height: 820 } });
        const errors = [];
        page.on('pageerror', e => errors.push(e.message));
        await page.route('**/*', route => new URL(route.request().url()).origin === base ? route.continue() : route.abort());
        await page.addInitScript(() => {
            localStorage.setItem('praxis_token', 'synthetic-audio-token');
            localStorage.setItem('praxis_chat_sessions', JSON.stringify([{ id: 'default', name: 'Default' }, { id: 'other', name: 'Other' }]));
        });
        await page.goto(base, { waitUntil: 'domcontentloaded' });
        await page.waitForFunction(() => chatTtsOn && chatTtsSideSources.length === 1);
        await page.evaluate(() => showTab('chat'));
        await page.fill('#chat-input', 'unlock via Send with restored TTS switch');
        await page.click('#chat-send-btn');
        await page.waitForFunction(() => chatTtsUnlocked);
        await page.evaluate(() => { window.unlockedPlayer = chatTtsAudio; });
        assert.equal(openings.default, 1);
        assert.equal(openings.discord, 1);

        const first = reply(101, 'First delayed reply');
        await page.waitForFunction(() => !isAgentActive && document.querySelector('.assistant[data-message-id="101"]'));
        await page.waitForTimeout(100); // TTS finishes after agent_stop, not with text
        assert.equal(openings.default, 1, 'agent_start must not reopen the main SSE');
        assert.equal(openings.discord, 1, 'agent lifecycle must not close the TTS side channel');
        ready(first);
        await playing(page, 101);
        assert(await page.evaluate(() => chatTtsAudio === window.unlockedPlayer), 'autoplay must reuse the unlocked media element');
        await page.evaluate(() => chatEventSource.onerror(new Event('error')));
        assert.equal(await page.evaluate(() => chatTtsState), 'playing', 'connection error must not stop existing audio');
        await idle(page);
        assert.equal(await button(page, 101).textContent(), '▶ Audio abspielen', 'button survives natural playback completion');
        await button(page, 101).click();
        await playing(page, 101);
        await page.click('#chat-tts-indicator');
        await idle(page);
        const seenRequests = audioRequests;
        ready(first); // duplicate SSE and history reports must not autoplay twice
        await page.evaluate(() => pollChatMessages());
        await page.waitForTimeout(100);
        assert.equal(audioRequests, seenRequests);

        await page.click('#chat-tts-btn'); // OFF only controls automatic playback
        await page.waitForFunction(() => !chatTtsOn);
        await button(page, 101).click();
        await playing(page, 101);
        await button(page, 101).click();
        await idle(page);
        await page.click('#chat-tts-btn');
        await page.waitForFunction(() => chatTtsOn);

        // Force a browser-policy rejection, then verify an ordinary keyboard-
        // accessible replay button retries the SAME ready clip synchronously.
        await page.evaluate(() => {
            window.realPlay = HTMLMediaElement.prototype.play;
            HTMLMediaElement.prototype.play = function () {
                if (this.src.startsWith('blob:')) return Promise.reject(new DOMException('fixture policy block', 'NotAllowedError'));
                return window.realPlay.call(this);
            };
        });
        const second = reply(102, 'Second delayed reply');
        ready(second);
        await page.waitForFunction(() => chatTtsState === 'ready');
        assert((await page.locator('.assistant[data-message-id="102"] .chat-audio-status').textContent()).includes('Autoplay blockiert'));
        await page.evaluate(() => { HTMLMediaElement.prototype.play = window.realPlay; });
        await button(page, 102).focus();
        await page.keyboard.press('Enter');
        await playing(page, 102);
        assert((await page.locator('#chat-tts-indicator').textContent()).includes('spricht'), 'ready chip must reset to speaking on retry');
        await idle(page);

        // History reload restores durable controls, but never autoplays old audio.
        const beforeReload = audioRequests;
        await page.reload({ waitUntil: 'domcontentloaded' });
        await page.waitForFunction(() => chatTtsSideSources.length === 1 && document.querySelectorAll('.assistant .chat-audio-play').length === 2);
        await page.waitForTimeout(150);
        assert.equal(audioRequests, beforeReload);
        await page.evaluate(() => showTab('chat'));
        await button(page, 101).click();
        await playing(page, 101);
        await idle(page);

        // Disconnect, produce audio without any subscriber, then recover via
        // history on EventSource's native reconnect. Side stream stays intact.
        const sideBeforeReconnect = openings.discord;
        for (const s of streams) if (s.uid === 'default') s.res.end();
        messages.default.push({ id: 103, role: 'assistant', content: 'Produced offline', audio_mime: 'audio/wav' });
        await playing(page, 103);
        await idle(page);
        assert.equal(openings.discord, sideBeforeReconnect);
        assert.equal(await button(page, 103).count(), 1);

        // A TTS event arriving on a paired side stream is never attached to the
        // active session's last reply, and retains its own replay button.
        const discord = { id: 201, role: 'assistant', content: 'Discord fixture', audio_mime: 'audio/wav' };
        messages.discord.push(discord);
        ready(discord, 'discord');
        await playing(page, 201);
        assert.equal(await page.locator('.chat-msg.system .chat-audio-play').count(), 1);
        await idle(page);

        // Back-to-back speech is queued rather than cutting off the first clip.
        // Identical reply text still belongs to two distinct durable identities.
        const queuedFirst = reply(105, 'Identical reply');
        ready(queuedFirst);
        await playing(page, 105);
        const queuedSecond = reply(106, 'Identical reply');
        ready(queuedSecond);
        await page.waitForFunction(() => chatTtsQueue.length === 1);
        assert.equal(await page.evaluate(() => chatTtsCurrent.messageId), 105);
        await playing(page, 106);
        await idle(page);
        assert.equal(await button(page, 105).count(), 1);
        assert.equal(await button(page, 106).count(), 1);

        failId = 107;
        const failed = reply(107, 'Recoverable audio fetch failure');
        ready(failed);
        await page.waitForFunction(() => chatTtsState === 'error');
        const hint = await page.locator('.assistant[data-message-id="107"] .chat-audio-status').textContent();
        assert(hint.includes('erneut versuchen') && !hint.includes('Autoplay'), 'network/media errors are not autoplay-policy errors');
        failId = null;
        await button(page, 107).click();
        await playing(page, 107);
        await idle(page);

        // Context changes outside the chat toggle must immediately cancel
        // pending bytes AND queued autoplay, without removing replay controls.
        delayId = 108;
        const disabling = reply(108, 'Disabled during audio fetch');
        ready(disabling);
        await page.waitForFunction(() => chatTtsState === 'loading' && chatTtsCurrent.messageId === 108);
        while (!delayedAudio) await new Promise(resolve => setTimeout(resolve, 10));
        ready(reply(109, 'Queued before context disable'));
        await page.waitForFunction(() => chatTtsQueue.length === 1);
        setTts('default', false);
        await page.waitForFunction(() => !chatTtsOn && chatTtsState === 'idle', null, { timeout: 3000 });
        assert.equal(await page.evaluate(() => chatTtsQueue.length), 0);
        assert.equal(await page.locator('#chat-tts-btn').getAttribute('aria-pressed'), 'false');
        delayedAudio.end(wav);
        delayedAudio = null;
        delayId = null;
        const afterDisable = audioRequests;
        ready(reply(110, 'Late notification after context disable'));
        await page.evaluate(() => pollChatMessages());
        await page.waitForTimeout(150);
        assert.equal(audioRequests, afterDisable, 'OFF blocks late SSE and history autoplay');
        await button(page, 108).click();
        await playing(page, 108);
        setTts('default', false); // unrelated context saves repeat the same flag
        await page.waitForTimeout(100);
        assert.equal(await page.evaluate(() => chatTtsState), 'playing', 'repeated OFF must not interrupt manual replay');
        await button(page, 108).click();
        await idle(page);

        // A GET started before an OFF event must not restore its stale ON value.
        setTts('default', true);
        await page.waitForFunction(() => chatTtsOn);
        delayContextUid = 'default';
        await page.evaluate(() => { window.pendingTtsLoad = loadTtsSwitchState(); });
        while (!delayedContext) await new Promise(resolve => setTimeout(resolve, 10));
        assert.equal(delayedContext.body.settings.web_chat_tts, true);
        setTts('default', false);
        await page.waitForFunction(() => !chatTtsOn);
        json(delayedContext.res, delayedContext.body);
        delayedContext = null;
        await page.evaluate(() => window.pendingTtsLoad);
        assert.equal(await page.evaluate(() => chatTtsOn), false, 'stale GET must not undo context disable');

        // A missed settings event is recovered before reconnect/history autoplay.
        setTts('default', true);
        await page.waitForFunction(() => chatTtsOn);
        const beforeMutedReconnect = audioRequests;
        const beforeMutedOpenings = openings.default;
        for (const s of streams) if (s.uid === 'default') s.res.end();
        setTts('default', false, false);
        messages.default.push({ id: 111, role: 'assistant', content: 'Offline while muted', audio_mime: 'audio/wav' });
        while (openings.default === beforeMutedOpenings) await new Promise(resolve => setTimeout(resolve, 10));
        await page.waitForFunction(() => !chatTtsOn && document.querySelector('.assistant[data-message-id="111"] .chat-audio-play'));
        await page.waitForTimeout(150);
        assert.equal(audioRequests, beforeMutedReconnect);

        // A paired Discord context has its own web-TTS permission. Disabling it
        // stops that source without muting the active chat or breaking replay.
        setTts('default', true);
        await page.waitForFunction(() => chatTtsOn);
        const sideReply = { id: 202, role: 'assistant', content: 'Side context disable' };
        messages.discord.push(sideReply);
        ready(sideReply, 'discord');
        await playing(page, 202);
        setTts('discord', false);
        await idle(page);
        assert.equal(await page.evaluate(() => chatTtsOn), true);
        const beforeMutedSide = audioRequests;
        const mutedSide = { id: 203, role: 'assistant', content: 'Muted side context' };
        messages.discord.push(mutedSide);
        ready(mutedSide, 'discord');
        await page.waitForTimeout(200);
        assert.equal(audioRequests, beforeMutedSide, 'side source OFF blocks late notifications');
        await page.locator(`.chat-audio-play[data-audio-key='${JSON.stringify(['discord', '202'])}']`).click();
        await playing(page, 202);
        await page.click('#chat-tts-indicator');
        await idle(page);

        // Even without an SSE setting notification, autoplay must check the
        // persisted context. A failed permission lookup must fail closed too.
        const beforeUncheckedAudio = audioRequests;
        setTts('default', false, false);
        ready(reply(112, 'Permission changed without a notification'));
        await page.waitForFunction(() => !chatTtsOn && chatTtsState === 'idle');
        assert.equal(audioRequests, beforeUncheckedAudio);
        setTts('default', true);
        await page.waitForFunction(() => chatTtsOn);
        failContextUid = 'default';
        ready(reply(113, 'Permission lookup unavailable'));
        await page.waitForFunction(() => !chatTtsOn && chatTtsState === 'idle');
        assert.equal(audioRequests, beforeUncheckedAudio);
        await button(page, 113).click();
        await playing(page, 113); // manual replay needs no autoplay permission
        await page.click('#chat-tts-indicator');
        await idle(page);
        failContextUid = null;
        setTts('default', true);
        await page.waitForFunction(() => chatTtsOn);

        // Switching sessions while bytes are still loading invalidates the old
        // request; the late completion must not start playback in the new chat.
        delayId = 104;
        const late = reply(104, 'Late fetch');
        ready(late);
        await page.waitForFunction(() => chatTtsState === 'loading' && chatTtsCurrent.messageId === 104);
        while (!delayedAudio) await new Promise(resolve => setTimeout(resolve, 10));
        await page.evaluate(() => switchChatSession('other'));
        delayedAudio.end(wav);
        await page.waitForFunction(() => chatStreamUserId === 'other' && chatTtsState === 'idle');
        assert.equal(await page.locator('.assistant .chat-audio-play').count(), 0);
        await page.waitForTimeout(150);
        assert.equal(await page.evaluate(() => chatTtsCurrent), null);
        delayContextUid = 'other';
        await page.evaluate(() => { window.pendingTtsLoad = loadTtsSwitchState(); });
        while (!delayedContext) await new Promise(resolve => setTimeout(resolve, 10));
        await page.evaluate(() => logout());
        json(delayedContext.res, delayedContext.body);
        delayedContext = null;
        await page.evaluate(() => window.pendingTtsLoad);
        assert.equal(await page.evaluate(() => chatTtsOn), false, 'late context response must not undo logout');
        await page.waitForTimeout(100);
        assert.equal(await page.evaluate(() => chatEventSource), null);
        assert.equal(await page.evaluate(() => chatTtsSideSources.length), 0);
        assert.deepEqual(errors, []);
        console.log('test_chat_audio: ok (delayed SSE, replay, autoplay policy, context disable, stale settings, history, reconnect, side channel, session isolation)');
    } finally {
        for (const s of streams) s.res.end();
        delayedAudio?.end();
        delayedContext?.res.end();
        if (browser) await browser.close();
        server.closeAllConnections();
        server.close();
    }
})().catch(err => { console.error(err); process.exitCode = 1; });
