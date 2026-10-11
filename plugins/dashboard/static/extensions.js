// Trusted first-party package pages. The host authenticates and validates each
// contribution; arbitrary third-party UI isolation is a later plugin API.
(function () {
    const entries = new Map();
    let actions = {}, active = null, generation = 0;
    const api = {
        configure(bridge) { actions = bridge; Object.assign(api, bridge); },
        register(id, hooks) { const entry=entries.get(id); if(entry) entry.hooks=hooks; },
        async load() {
            const current = generation;
            const response = await actions.apiGet('/api/dashboard/extensions');
            if(!response.ok) return;
            const data = await response.json();
            if(current !== generation) return;
            // Registered slots without a live contribution explain how to get
            // the feature. They own no page and never execute package code.
            const slots=new Map();
            for(const slot of data.slots || []) {
                const id=slot && typeof slot.id==='string' ? slot.id : '';
                if(!/^[a-zA-Z0-9_-]{1,64}$/.test(id) || slot.present) continue;
                slots.set(id,{
                    title: typeof slot.title==='string' && slot.title ? slot.title : id,
                    hint: typeof slot.hint==='string' && slot.hint ? slot.hint : 'Install the package to use this feature.'
                });
            }
            // A placeholder must neither block a real contribution nor outlive it.
            for(const [id,entry] of [...entries]) {
                if(!entry.absent || slots.has(id)) continue;
                if(active===id) { api.hideAll(); actions.showTab('overview'); }
                remove(id,entry);
            }
            const live = new Set();
            for(const descriptor of data.extensions || []) {
                const id=descriptor.id;
                if(typeof id!=='string' || !/^[a-zA-Z0-9_-]{1,64}$/.test(id) || typeof descriptor.title!=='string') continue;
                const prefix=`/plugins/${id}/`;
                const safe=path=>typeof path==='string' && path.startsWith(prefix) && /^[a-zA-Z0-9/_\-.]+$/.test(path) && !path.split('/').some(part=>part==='.'||part==='..');
                if(![descriptor.page,descriptor.script,descriptor.style].every(safe)) continue;
                live.add(id);
                if(entries.has(id)) continue;
                if(document.getElementById(`tab-${id}`)) continue;
                const nav=document.createElement('li'); nav.dataset.tab=id; nav.textContent=descriptor.title;
                nav.addEventListener('click',()=>actions.showTab(id));
                document.querySelector('.nav-links').appendChild(nav);
                const panel=document.createElement('div'); panel.id=`tab-${id}`; panel.classList.add('tab');
                document.querySelector('.content').appendChild(panel);
                entries.set(id,{descriptor,nav,panel,hooks:null,promise:null,mounted:false,script:null,style:null});
            }
            for(const [id,entry] of entries) if(!live.has(id) && !entry.absent) { if(active===id) { api.hideAll(); actions.showTab('overview'); } remove(id,entry); }
            for(const [id,slot] of slots) {
                if(live.has(id) || entries.has(id) || document.getElementById(`tab-${id}`)) continue;
                const nav=document.createElement('li'); nav.dataset.tab=id; nav.textContent=slot.title;
                nav.classList.add('absent'); nav.title=slot.hint;
                nav.addEventListener('click',()=>actions.showTab(id));
                document.querySelector('.nav-links').appendChild(nav);
                const panel=document.createElement('div'); panel.id=`tab-${id}`; panel.classList.add('tab'); panel.classList.add('absent');
                panel.textContent=slot.hint;
                document.querySelector('.content').appendChild(panel);
                entries.set(id,{descriptor:null,nav,panel,hooks:null,promise:null,mounted:false,script:null,style:null,absent:true});
            }
        },
        async show(id) {
            const entry=entries.get(id); if(!entry || entry.absent) return;
            active=id;
            if(!entry.promise) entry.promise=(async()=>{
                const response=await actions.apiGet(entry.descriptor.page);
                if(!response.ok) throw new Error('Package page unavailable');
                const html=await response.text();
                if(entries.get(id)!==entry) return;
                entry.panel.innerHTML=html;
                entry.style=document.createElement('link'); entry.style.rel='stylesheet'; entry.style.href=entry.descriptor.style;
                document.head.appendChild(entry.style);
                await new Promise((resolve,reject)=>{
                    entry.script=document.createElement('script'); entry.script.src=entry.descriptor.script;
                    entry.script.onload=resolve; entry.script.onerror=()=>reject(new Error('Package script unavailable'));
                    document.head.appendChild(entry.script);
                });
                if(entries.get(id)===entry && !entry.hooks) throw new Error('Package did not register its page');
            })().catch(error=>{
                entry.script?.remove(); entry.style?.remove(); entry.promise=null;
                if(entries.get(id)===entry) entry.panel.textContent=error.message;
                throw error;
            });
            try {
                await entry.promise;
                if(active===id && entries.get(id)===entry && !entry.mounted) {
                    entry.mounted=true;
                    await entry.hooks?.onShow?.();
                }
            } catch(error) { console.error('Dashboard contribution:',error); }
        },
        hideAll() {
            active=null;
            for(const entry of entries.values()) if(entry.mounted) { entry.mounted=false; entry.hooks?.onHide?.(); }
        },
        reset() { generation++; api.hideAll(); for(const [id,entry] of entries) remove(id,entry); }
    };
    function remove(id,entry) {
        if(entry.mounted) entry.hooks?.onHide?.();
        if(active===id) active=null;
        entry.nav.remove();entry.panel.remove();entry.script?.remove();entry.style?.remove(); entries.delete(id);
    }
    window.PraxisDashboard=api;
})();
