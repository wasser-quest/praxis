#!/usr/bin/env node
/* Synthetic UI regression checks. No network, credentials, or services. */
const assert = require('assert');
const fs = require('fs');
const path = require('path');

const root = path.resolve(__dirname, '..');
const html = fs.readFileSync(path.join(root, 'static/index.html'), 'utf8');
const css = fs.readFileSync(path.join(root, 'static/style.css'), 'utf8');
const js = fs.readFileSync(path.join(root, 'static/app.js'), 'utf8');

assert(html.includes('href="/favicon.ico"'), 'index must use /favicon.ico');
assert(html.includes('href="/apple-touch-icon.png"'), 'index must use /apple-touch-icon.png');
assert(html.includes('src="/logo.svg"'), 'branding must reference the editable SVG master');
assert(!html.includes('/logo.png') && !js.includes('/logo.png'), 'dashboard fallbacks must use the SVG master');
assert(js.includes("this.onerror=null;this.src='/logo.svg'"), 'failed custom avatars need a bounded brand fallback');
assert(fs.readFileSync(path.join(root, 'static/logo.svg')).equals(fs.readFileSync(path.join(root, 'homepage/assets/logo.svg'))), 'homepage and dashboard must share one logo');

assert(/function\s+contextSmFile\s*\(ctx\)\s*{\s*return ctx\?\.sm_file \|\| ctx\?\.cl_file \|\| '';/s.test(js),
  'UI must prefer canonical sm_file while retaining cl_file fallback');
assert(js.includes('/api/sm/${encodedUser}') && js.includes('/api/cl/${encodedUser}'), 'SM status should prefer canonical route with legacy fallback');
assert(js.includes('Statemachine: ${smFile}'), 'SM status badge should use Statemachine label');
assert(js.includes("'sm_file', 'cl_file'"), 'context redundant keys should include canonical and legacy state-machine keys');

assert(js.includes('class="data-item tool-item"'), 'tools list must use wrapping-friendly tool item class');
assert(js.includes('type="button" class="toggle'), 'tool toggles should render as buttons, not anonymous divs');
assert(js.includes('aria-pressed="${t.is_enabled ? \'true\' : \'false\'}"'), 'tool toggles need aria-pressed state');
assert(/onclick='toggleTool\(\$\{nameArg\}, \$\{!t\.is_enabled\}\)'/.test(js), 'tool toggle onclick must use JSON-escaped name argument');

for (const needle of [
  'body { font-family:',
  'overflow-x: hidden',
  '.content { margin-left: 250px',
  'width: calc(100% - 250px)',
  '.tool-copy { min-width: 0; flex: 1; }',
  'overflow-wrap: anywhere',
  '@media (max-width: 768px)',
  '.chat-app, .content.chat-expanded .chat-app { flex-direction: column; height: calc(100dvh - 44px); min-height: 0; overflow: hidden; }',
  '.content.chat-expanded, .sidebar.collapsed ~ .content.chat-expanded { padding: 44px 0 0; height: 100dvh; overflow: hidden; }',
  'body.chat-conversations-open .chat-sidebar { display: flex; position: fixed;',
  '[data-theme="light"] .chat-msg.user .msg-content',
]) {
  assert(css.includes(needle), `CSS missing expected rule fragment: ${needle}`);
}

assert(js.includes("function contextSmData(ctx)"), 'canonical workflow data helper missing');
assert(js.includes("'sm_data'"), 'UI must prefer sm_data over legacy data');
assert(!html.includes('cl-status') && !css.includes('.cl-badge'), 'legacy workflow DOM/CSS identifiers remain');
console.log('test_ui_static: ok');
