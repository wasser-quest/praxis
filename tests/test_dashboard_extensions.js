// No browser session or model inference required.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

class Element {
    constructor(tag) { this.tagName=tag; this.children=[]; this.classList={add(){},remove(){}}; this.dataset={}; this.textContent=''; this.innerHTML=''; }
    appendChild(child) { this.children.push(child); child.parent=this; if(child.tagName==='script') { scripts++; context.PraxisDashboard.register('vm',{onShow(){shows++},onHide(){hides++}}); queueMicrotask(()=>child.onload()); } return child; }
    remove() { if(this.parent) this.parent.children=this.parent.children.filter(child=>child!==this); }
    addEventListener(type, fn) { this[type]=fn; }
}
let scripts=0, shows=0, hides=0;
const nav=new Element('ul'), content=new Element('div'), head=new Element('head');
const document={head,createElement:tag=>new Element(tag),getElementById:id=>[...content.children].find(child=>child.id===id),querySelector:selector=>selector==='.nav-links'?nav:selector==='.content'?content:null};
const context={document,console,window:{},URL,Promise,queueMicrotask};
vm.createContext(context);
vm.runInContext(fs.readFileSync('static/extensions.js','utf8'),context);
context.PraxisDashboard=context.window.PraxisDashboard;
const valid={id:'vm',title:'Virtual machines',page:'/plugins/vm/ui/page.html',script:'/plugins/vm/ui/vm.js',style:'/plugins/vm/ui/vm.css'};
let descriptors=[valid];
context.PraxisDashboard.configure({apiGet:async path=>({ok:true,json:async()=>({extensions:descriptors}),text:async()=>'<h2>Package VM</h2>'}),showTab(){}});
(async()=>{
    await context.PraxisDashboard.load();
    assert.equal(nav.children.length,1);
    assert.equal(scripts,0,'loading metadata must not execute package code');
    await Promise.all([context.PraxisDashboard.show('vm'),context.PraxisDashboard.show('vm')]);
    assert.equal(scripts,1,'concurrent tab opens load code once');
    assert.equal(shows,1,'concurrent tab opens mount once');
    context.PraxisDashboard.hideAll();
    assert.equal(hides,1);
    context.PraxisDashboard.reset();
    assert.equal(nav.children.length,0);
    assert.equal(content.children.length,0);
    descriptors=[{...valid,page:'/api/contexts'}, {...valid,id:'chat'}, {...valid,script:'https://evil.test/code.js'}, {...valid,page:'/plugins/vm/../secret'}];
    await context.PraxisDashboard.load();
    assert.equal(nav.children.length,0,'reject contributions outside package namespace');
    descriptors=[];
    await context.PraxisDashboard.load();
    assert.equal(nav.children.length,0,'no package means no page');
    assert(!fs.readFileSync('static/index.html','utf8').includes('id="tab-vm"'));
    assert(!fs.readFileSync('static/app.js','utf8').includes('function loadVM('));
    // An absent feature keeps its navigation slot and explains installation.
    let slots=[{id:'vm',title:'Virtual Machines',present:false,hint:'Install the VM package'}];
    const base=scripts;
    const noPage=async()=>{ throw new Error('an absent feature must not load a page'); };
    context.PraxisDashboard.reset();
    context.PraxisDashboard.configure({apiGet:async()=>({ok:true,json:async()=>({extensions:[],slots}),text:noPage}),showTab(){}});
    await context.PraxisDashboard.load();
    assert.equal(nav.children.length,1,'an absent feature keeps its navigation slot');
    assert.equal(nav.children[0].textContent,'Virtual Machines');
    assert.equal(content.children[0].textContent,'Install the VM package','the slot explains installation');
    assert.equal(scripts,base,'an absent feature must not execute package code');
    await context.PraxisDashboard.show('vm');
    assert.equal(scripts,base,'an absent feature has no page to load');
    // A live contribution replaces its placeholder instead of duplicating it.
    slots=[{id:'vm',title:'Virtual Machines',present:true}];
    context.PraxisDashboard.configure({apiGet:async()=>({ok:true,json:async()=>({extensions:[valid],slots}),text:async()=>'<h2>Package VM</h2>'}),showTab(){}});
    await context.PraxisDashboard.load();
    assert.equal(nav.children.length,1,'the placeholder is replaced, not duplicated');
    assert.equal(nav.children[0].textContent,'Virtual machines');
    assert.equal(content.children.length,1);
    // Going away again brings the placeholder back.
    context.PraxisDashboard.configure({apiGet:async()=>({ok:true,json:async()=>({extensions:[],slots:[{id:'vm',title:'Virtual Machines',present:false,hint:'Install the VM package'}]}),text:noPage}),showTab(){}});
    await context.PraxisDashboard.load();
    assert.equal(nav.children.length,1,'the placeholder returns when the feature goes away');
    assert.equal(nav.children[0].textContent,'Virtual Machines');
    context.PraxisDashboard.reset();
    console.log('Dashboard extension lifecycle and namespace tests passed');
})().catch(error=>{ console.error(error);process.exitCode=1; });
