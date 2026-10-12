#!/usr/bin/env node
// Execute the actual streaming UI handler against a tiny synthetic DOM/EventSource.
// No browser, network, credentials or running Praxis instance is used.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const source = fs.readFileSync(path.join(__dirname, '../static/app.js'), 'utf8');
const start = source.indexOf('function startChatStream() {');
const end = source.indexOf('\nfunction stopChatStream()', start);
assert(start >= 0 && end > start);
const nodes = [];
const feedback = [];
let es;
const container = { querySelector: () => null, appendChild: node => nodes.push(node) };
const context = {
  chatUserId: 'synthetic', authToken: 'not-a-credential', chatBotName: 'Test', chatUsername: 'User',
  chatSeenIds: new Set(), chatEventSource: null,
  console: { log() {}, warn() {}, error(...args) { throw new Error(args.join(' ')); } },
  performance: { now: () => 0 },
  EventSource: class {
    constructor() { this.handlers = {}; es = this; }
    addEventListener(name, handler) { this.handlers[name] = handler; }
  },
  document: {
    body: { classList: { contains: () => false, add() {}, remove() {} } },
    getElementById: () => container,
    createElement() {
      const classes = new Set();
      const content = { innerHTML: '', classList: { remove(c) { classes.delete(c); }, toggle(c, on) { on ? classes.add(c) : classes.delete(c); } },
        querySelector() { return this.box || null; }, replaceChildren(box) { this.box = box; } };
      const label = { textContent: '' };
      return { content, label, style: {}, classList: content.classList, get isConnected() { return nodes.includes(this); },
        querySelector(selector) { return selector === '.msg-label' ? label : this.innerHTML?.includes('msg-content') ? content : null; },
        remove() { nodes.splice(nodes.indexOf(this), 1); } };
    }
  },
  stopChatStream() {}, stopChatTimer() {}, autoScrollChat() {},
  addAgentLoopBanner() {}, updateAgentUI() {},
  escapeHtml: text => text, renderMarkdown: text => text,
  addChatMessage: (role, text) => feedback.push([role, text]),
};
const constructorStart = source.indexOf('function addChatMessage(type, content, extra = null) {');
const constructorEnd = source.indexOf('\n}', constructorStart) + 2;
// Keep the actual constructor and handlers in the same VM scope.
context.recordFeedback = context.addChatMessage;
vm.runInNewContext(source.slice(constructorStart, constructorEnd) + '\nconst realMessage = addChatMessage; addChatMessage = (role, text) => { if (role === "thinking" || role === "tool-preview") return realMessage(role, text); return recordFeedback(role, text); };\n' + source.slice(start, end) + '\nstartChatStream();', context);
function emit(name, text = '') { assert(es.handlers[name], `missing event ${name}`); es.handlers[name]({ data: JSON.stringify({ data: text }) }); }
emit('char', 'unfinished old reply');
assert.equal(nodes.length, 1);
emit('feedback', 'LLM failed; progress retained');
emit('stream_abort');
assert.equal(nodes.length, 0, 'failed provisional reply must be removed');
emit('char', 'new ');
emit('char', 'reply');
assert.equal(nodes[0].content.innerHTML, 'new reply', 'old stream buffer must be cleared');
emit('assistant', 'new reply');
emit('stream_abort');
assert.equal(nodes.length, 1, 'a later abort must not erase a committed reply');
assert.equal(nodes[0].content.innerHTML, 'new reply');
assert.equal(feedback[0][0], 'feedback');
emit('stream_start');
emit('reasoning_delta', 'Checking ');
emit('reasoning_delta', '<script>source</script>');
const thinking = nodes.at(-1);
assert.equal(thinking.className, 'chat-msg thinking');
assert.equal(thinking.content.box.textContent, 'Checking <script>source</script>', 'reasoning is streamed as safe text');
assert.match(thinking.label.textContent, /Thinking/);
emit('tool_call_delta', JSON.stringify({index:0, name:'read_', arguments:'{'}));
emit('tool_call_delta', JSON.stringify({index:0, name:'file', arguments:'"path":"x"}'}));
const tool = nodes.at(-1);
assert.match(tool.label.textContent, /read_file.*generating/);
assert.equal(tool.content.box.textContent, '{"path":"x"}');
emit('stream_end');
assert.match(tool.label.textContent, /received, not yet executed/);
emit('stream_abort');
assert.match(tool.label.textContent, /aborted, not executed/);
console.log('test_llm_stream: ok');
