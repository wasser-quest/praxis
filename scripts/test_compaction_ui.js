#!/usr/bin/env node
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync(require('node:path').join(__dirname, '../static/app.js'), 'utf8');
const start = source.indexOf('async function chatCompactCommand() {');
const end = source.indexOf('\n// /skill:', start);
async function run(status, body) {
    const messages = [];
    let reloads = 0;
    const context = {chatUserId:'fixture', addChatMessage:(role,text)=>messages.push({role,text}),
        apiFetch:async()=>({ok:status >= 200 && status < 300, status, text:async()=>body}),
        loadChatHistory:async()=>reloads++};
    vm.runInNewContext(source.slice(start,end), context);
    await context.chatCompactCommand();
    return {messages,reloads};
}
(async () => {
    for (const [status, body] of [[500,''],[502,'<html>proxy error</html>'],[503,JSON.stringify({error:'Gateway unavailable'})]]) {
        const result = await run(status,body);
        assert.equal(result.reloads,0);
        assert.match(result.messages.at(-1).text,/compact failed/);
        assert.doesNotMatch(result.messages.at(-1).text,/JSON.parse|unexpected end/);
    }
    const success = await run(200,JSON.stringify({success:true,summary:'Saved goal'}));
    assert.equal(success.reloads,1);
    assert.match(success.messages.at(-1).text,/Saved goal/);
    for (const id of ['ctx-compaction-enabled','ctx-compaction-limit','ctx-history-limit','ctx-save-budgets']) assert(source.includes(id));
    console.log('test_compaction_ui: ok');
})().catch(error=>{ console.error(error); process.exitCode=1; });
