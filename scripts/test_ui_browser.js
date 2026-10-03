#!/usr/bin/env node
/* Browser UI regression test with synthetic local API fixtures.
 * No live Praxis service, DB, credentials, or provider calls are used.
 * If Playwright is not already available, install it under /tmp/praxis-ui-browser
 * and set PLAYWRIGHT_BROWSERS_PATH=/tmp/praxis-ui-browser-cache.
 */
const assert = require('assert');
const fs = require('fs');
const http = require('http');
const path = require('path');

function loadPlaywright() {
  const candidates = [
    'playwright',
    '/workspace/poml/node_modules/playwright',
    '/workspace/poml/node_modules/@playwright/test',
    '/tmp/praxis-ui-browser/node_modules/playwright',
  ];
  for (const mod of candidates) {
    try {
      const pw = require(mod);
      return pw.chromium ? pw : { chromium: pw.chromium };
    } catch (_) {}
  }
  throw new Error('Playwright not found. Tried repo, /workspace/poml/node_modules, and /tmp/praxis-ui-browser/node_modules.');
}

const ROOT = path.resolve(__dirname, '..');
const STATIC = path.resolve(process.env.PRAXIS_TEST_STATIC_DIR || path.join(ROOT, 'static'));
// Can exercise updated disk assets with HTML embedded by an older binary.
const INDEX = path.resolve(process.env.PRAXIS_TEST_INDEX_FILE || path.join(STATIC, 'index.html'));
const OUT = process.env.PRAXIS_UI_TEST_OUTPUT || '/tmp/praxis-poml-audit/ui';
fs.mkdirSync(OUT, { recursive: true });

let tools = [
  {
    name: 'execute_terminal_with_a_very_long_name_to_exercise_wrapping',
    description: 'Synthetic fixture: ' + 'very long tool description '.repeat(35) + 'END',
    is_enabled: true,
  },
  { name: 'write_file', description: 'Short description', is_enabled: false },
];

function json(res, obj, status = 200) {
  res.writeHead(status, { 'content-type': 'application/json', 'access-control-allow-origin': '*' });
  res.end(JSON.stringify(obj));
}

function serveFile(res, file) {
  if (file !== INDEX && !file.startsWith(STATIC + path.sep)) { res.writeHead(403).end('forbidden'); return; }
  fs.readFile(file, (err, data) => {
    if (err) { res.writeHead(404).end('not found'); return; }
    const ext = path.extname(file).toLowerCase();
    const types = { '.html': 'text/html', '.css': 'text/css', '.js': 'application/javascript', '.png': 'image/png', '.ico': 'image/x-icon', '.svg': 'image/svg+xml' };
    res.writeHead(200, { 'content-type': types[ext] || 'application/octet-stream' });
    res.end(data);
  });
}

function makeServer() {
  return http.createServer((req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1');
    if (req.method === 'OPTIONS') return res.writeHead(204).end();
    if (url.pathname === '/' || url.pathname === '/index.html') return serveFile(res, INDEX);
    if (url.pathname.startsWith('/static/')) return serveFile(res, path.join(STATIC, url.pathname.replace('/static/', '')));
    if (['/logo.svg', '/logo.png', '/favicon.ico', '/favicon.png', '/apple-touch-icon.png'].includes(url.pathname)) return serveFile(res, path.join(STATIC, path.basename(url.pathname)));
    if (url.pathname.startsWith('/api/avatar/')) return serveFile(res, path.join(STATIC, 'logo.png'));
    if (url.pathname === '/api/auth/login') return json(res, { token: 'synthetic-token' });
    if (url.pathname === '/api/status') return json(res, { version: 'browser-fixture' });
    if (url.pathname === '/api/contexts') return json(res, { contexts: [{ user_id: 'default', updated_at: '2026-01-01T00:00:00Z' }] });
    if (url.pathname === '/api/contexts/default') return json(res, { context: { user_id: 'default', username: 'Browser Tester', sm_file: 'standard.sm', settings: { agent_name: 'Praxis' } } });
    if (url.pathname === '/api/pairings') return json(res, { pairings: [] });
    if (url.pathname === '/api/pairings/pending') return json(res, { pending_pairings: [] });
    if (url.pathname === '/api/tools/all' && req.method === 'GET') return json(res, { tools });
    if (url.pathname.startsWith('/api/tools/') && req.method === 'PUT') {
      let body = '';
      req.on('data', c => body += c);
      req.on('end', () => {
        const name = decodeURIComponent(url.pathname.split('/').pop());
        const patch = body ? JSON.parse(body) : {};
        tools = tools.map(t => t.name === name ? { ...t, is_enabled: !!patch.is_enabled } : t);
        json(res, { success: true });
      });
      return;
    }
    if (url.pathname === '/api/messages/default') return json(res, { messages: [
      { role: 'user', content: 'hello from fixture' },
      { role: 'assistant', content: 'Assistant response with **markdown** and a long token abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz.' },
      { role: 'tool', tool_name: 'fixture_tool', content: 'tool output '.repeat(30) },
    ], message_count: 3, total_tokens: 42 });
    if (url.pathname === '/api/chat/send' && req.method === 'POST') return json(res, { type: 'queued' });
    if (url.pathname === '/api/sm/default' || url.pathname === '/api/cl/default') return json(res, { sm_file: 'standard.sm', active_state: 'routing', system_template: 'language_instructor', active_skill: 'poml_templates', active_templates: [], sm_data: { mode: 'fixture' } });
    if (url.pathname === '/api/agent/status/default') return json(res, { active: false });
    if (url.pathname === '/api/chat/stream/default') {
      res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache', connection: 'keep-alive' });
      res.write(': synthetic stream\n\n');
      return;
    }
    json(res, {}, 404);
  });
}

async function noPageOverflow(page, label) {
  const metrics = await page.evaluate(() => ({
    iw: window.innerWidth,
    sw: document.documentElement.scrollWidth,
    bw: document.body.scrollWidth,
    chatSW: document.querySelector('.chat-messages')?.scrollWidth || 0,
    chatCW: document.querySelector('.chat-messages')?.clientWidth || 0,
  }));
  assert(metrics.sw <= metrics.iw + 2, `${label}: document overflow ${metrics.sw} > ${metrics.iw}`);
  assert(metrics.bw <= metrics.iw + 2, `${label}: body overflow ${metrics.bw} > ${metrics.iw}`);
  if (metrics.chatCW) assert(metrics.chatSW <= metrics.chatCW + 2, `${label}: chat overflow ${metrics.chatSW} > ${metrics.chatCW}`);
}

async function assertComposerVisible(page, label) {
  const geom = await page.evaluate(() => {
    const row = document.querySelector('.chat-input-row')?.getBoundingClientRect();
    const input = document.querySelector('#chat-input')?.getBoundingClientRect();
    const send = document.querySelector('#chat-send-btn')?.getBoundingClientRect();
    return { row, input, send, ih: innerHeight, iw: innerWidth };
  });
  for (const [name, rect] of [['row', geom.row], ['input', geom.input], ['send', geom.send]]) {
    assert(rect, `${label}: missing ${name}`);
    assert(rect.top >= -1, `${label}: ${name} top clipped (${rect.top})`);
    assert(rect.bottom <= geom.ih + 1, `${label}: ${name} bottom clipped (${rect.bottom} > ${geom.ih})`);
    assert(rect.width > 12 && rect.height > 12, `${label}: ${name} too small/click target invalid`);
  }
  await page.locator('#chat-send-btn').click({ trial: true });
}

async function assertMessagesScrollIndependently(page, label) {
  const result = await page.evaluate(() => {
    for (let i = 0; i < 35; i++) addChatMessage(i % 2 ? 'assistant' : 'user', `scroll proof ${i} ${'m'.repeat(80)}`);
    const box = document.querySelector('.chat-messages');
    const composerBefore = document.querySelector('.chat-input-row').getBoundingClientRect();
    box.scrollTop = 0;
    void box.offsetHeight;
    const top = box.scrollTop;
    box.scrollTo({ top: box.scrollHeight, behavior: 'instant' });
    void box.offsetHeight;
    const bottom = box.scrollTop;
    const last = box.lastElementChild;
    const lastText = last?.textContent || '';
    const lastRect = last?.getBoundingClientRect();
    const boxRect = box.getBoundingClientRect();
    const composerAfter = document.querySelector('.chat-input-row').getBoundingClientRect();
    return {
      scrollable: box.scrollHeight > box.clientHeight + 20,
      moved: bottom > top,
      lastText: lastText.slice(0, 160),
      lastBottomVisible: !!lastRect && lastRect.bottom <= boxRect.bottom + 2 && lastRect.bottom > boxRect.top,
      composerStable: Math.abs(composerBefore.top - composerAfter.top) < 1 && Math.abs(composerBefore.bottom - composerAfter.bottom) < 1,
      composerBottom: composerAfter.bottom,
      ih: innerHeight,
    };
  });
  assert(result.scrollable, `${label}: messages pane is not independently scrollable`);
  assert(result.moved, `${label}: messages scrollTop did not move`);
  assert(result.lastText.includes('scroll proof 34'), `${label}: last synthetic message not last child (${result.lastText})`);
  assert(result.lastBottomVisible, `${label}: last synthetic message not visible after scrolling to bottom`);
  assert(result.composerStable, `${label}: composer moved during messages scroll`);
  assert(result.composerBottom <= result.ih + 1, `${label}: composer below viewport after scroll`);
}

async function exerciseMobileMenus(page, width) {
  if (width > 768) return;
  await page.click('#chat-sidebar-toggle');
  await page.waitForSelector('body.chat-conversations-open .chat-sidebar');
  const convOpen = await page.locator('body.chat-conversations-open .chat-sidebar').boundingBox();
  assert(convOpen && convOpen.width > 150, `${width}: conversations sidebar did not open`);
  await page.click('#chat-sidebar-toggle');
  await page.waitForFunction(() => !document.body.classList.contains('chat-conversations-open'));

  await page.click('#sidebar .sidebar-toggle');
  await page.waitForFunction(() => !document.querySelector('#sidebar').classList.contains('collapsed'));
  const navVisible = await page.locator('#sidebar .nav-links').boundingBox();
  assert(navVisible && navVisible.height > 20, `${width}: mobile app menu did not open`);
  await page.click('#sidebar .sidebar-toggle');
  await page.waitForFunction(() => document.querySelector('#sidebar').classList.contains('collapsed'));
}

async function returnFromChatToDashboard(page, label) {
  // A CSS class change is not enough: the Chats button used to cover the
  // dashboard toggle on desktop and the Overview link on mobile.
  await page.locator('#sidebar-toggle').click({ timeout: 5000 });
  await page.waitForFunction(() => !document.querySelector('#sidebar').classList.contains('collapsed'));
  await page.locator('[data-tab="overview"]').click({ timeout: 5000 });
  await page.waitForSelector('#tab-overview.active');
  assert(!(await page.locator('.content').evaluate(el => el.classList.contains('chat-expanded'))), `${label}: chat fullscreen state was not cleared`);
  await noPageOverflow(page, `${label} returned overview`);
  await page.locator('[data-tab="tools"]').click();
  await page.waitForSelector('#tab-tools.active');
  await page.locator('[data-tab="chat"]').click();
  await page.waitForSelector('#tab-chat.active');
  await assertComposerVisible(page, `${label} reentered chat`);
}

async function runViewport(browser, baseURL, width, height, theme = 'dark') {
  const page = await browser.newPage({ viewport: { width, height }, deviceScaleFactor: 1 });
  page.on('pageerror', err => { throw err; });
  await page.route('**/*', route => new URL(route.request().url()).origin === new URL(baseURL).origin
    ? route.continue() : route.abort());
  await page.goto(baseURL, { waitUntil: 'domcontentloaded' });
  assert(await page.locator('.logo-img').evaluate(async img => {
    try { await img.decode(); return img.naturalWidth > 0; } catch { return false; }
  }), `${width}: login logo could not be decoded`);
  for (const [file, type] of [['/logo.svg', 'image/svg+xml'], ['/logo.png', 'image/png'], ['/favicon.ico', 'image/x-icon'], ['/apple-touch-icon.png', 'image/png']]) {
    const response = await page.request.get(new URL(file, baseURL).href);
    assert(response.ok() && response.headers()['content-type'].startsWith(type), `${file}: missing asset or wrong MIME type`);
  }
  await page.evaluate(t => { localStorage.setItem('praxis-theme', t); document.body.setAttribute('data-theme', t); }, theme);
  await page.fill('#password', 'anything');
  await page.click('#login-form button[type="submit"]');
  await page.waitForSelector('#dashboard-screen.active');
  await page.waitForTimeout(250);
  await noPageOverflow(page, `${width}x${height} ${theme} overview`);
  await page.screenshot({ path: path.join(OUT, `praxis-overview-${width}x${height}-${theme}.png`), fullPage: false });

  await page.click('[data-tab="tools"]');
  await page.waitForSelector('#tools-list .tool-item');
  await noPageOverflow(page, `${width}x${height} ${theme} tools`);
  const geom = await page.locator('#tools-list .tool-item').first().evaluate(el => {
    const item = el.getBoundingClientRect();
    const meta = el.querySelector('.meta').getBoundingClientRect();
    const toggle = el.querySelector('.toggle').getBoundingClientRect();
    return { item, meta, toggle, vw: innerWidth };
  });
  assert(geom.toggle.width >= 44 && geom.toggle.width <= 54, `${width}x${height} tools: toggle width ${geom.toggle.width}`);
  assert(geom.toggle.right <= geom.vw + 1, `${width}x${height} tools: toggle extends past viewport`);
  assert(geom.meta.width > 0 && geom.meta.right <= geom.vw + 1, `${width}x${height} tools: description geometry invalid`);
  const before = await page.locator('#tools-list .tool-item .toggle').first().getAttribute('aria-pressed');
  await page.locator('#tools-list .tool-item .toggle').first().click();
  await page.waitForTimeout(150);
  const after = await page.locator('#tools-list .tool-item .toggle').first().getAttribute('aria-pressed');
  assert.notStrictEqual(after, before, `${width}x${height} tools: toggle click did not update aria-pressed`);
  await page.screenshot({ path: path.join(OUT, `praxis-tools-${width}x${height}-${theme}.png`), fullPage: false });

  await page.click('[data-tab="chat"]');
  await page.waitForSelector('.chat-container');
  await page.waitForFunction(() => document.querySelector('#sm-status-temps')?.textContent === 'System: language_instructor');
  assert.strictEqual(await page.locator('#sm-status-skill').textContent(), 'Skill: poml_templates');
  assert((await page.locator('#sm-status-vars').textContent()).includes('fixture'), 'canonical sm_data not displayed');
  await page.evaluate(() => {
    addChatMessage('user', 'user-long-' + 'x'.repeat(220));
    addChatMessage('assistant', 'assistant markdown with a long token ' + 'y'.repeat(220) + '\n\n```txt\n' + 'z'.repeat(160) + '\n```');
  });
  await page.waitForTimeout(250);
  await noPageOverflow(page, `${width}x${height} ${theme} chat`);
  await assertComposerVisible(page, `${width}x${height} ${theme} chat initial`);
  await exerciseMobileMenus(page, width);
  await assertComposerVisible(page, `${width}x${height} ${theme} chat after menus`);
  await assertMessagesScrollIndependently(page, `${width}x${height} ${theme} chat scroll`);
  await assertComposerVisible(page, `${width}x${height} ${theme} chat after scroll`);
  const chatGeom = await page.locator('.chat-app').evaluate(el => {
    const r = el.getBoundingClientRect();
    const input = document.querySelector('.chat-input-row').getBoundingClientRect();
    return { right: r.right, width: r.width, inputRight: input.right, vw: innerWidth };
  });
  assert(chatGeom.right <= chatGeom.vw + 1, `${width}x${height} chat: app extends past viewport`);
  assert(chatGeom.inputRight <= chatGeom.vw + 1, `${width}x${height} chat: input row extends past viewport`);
  await page.waitForFunction(() => [...document.querySelectorAll('#tab-chat .msg-avatar')]
    .every(img => img.complete && img.naturalWidth > 0), null, { timeout: 10000 });
  await page.screenshot({ path: path.join(OUT, `praxis-chat-${width}x${height}-${theme}.png`), fullPage: false });
  await returnFromChatToDashboard(page, `${width}x${height} ${theme}`);
  await page.close();
}

(async () => {
  const { chromium } = loadPlaywright();
  const server = makeServer();
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const baseURL = `http://127.0.0.1:${server.address().port}/`;
  let browser;
  try {
    browser = await chromium.launch({ headless: true });
    for (const vp of [
      [360, 640, 'dark'],
      [360, 640, 'light'],
      [390, 820, 'dark'],
      [390, 820, 'light'],
      [768, 900, 'dark'],
      [1280, 800, 'dark'],
    ]) {
      await runViewport(browser, baseURL, ...vp);
    }
    console.log(`test_ui_browser: ok screenshots=${OUT}`);
  } finally {
    if (browser) await browser.close();
    server.close();
  }
})().catch(err => {
  console.error('test_ui_browser: FAIL');
  console.error(err && err.stack || err);
  process.exit(1);
});
