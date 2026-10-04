(function () {
const { apiGet, apiFetch, escapeHtml, showModal, closeModal } = PraxisDashboard;
let vmVisible = false, vmGeneration = 0;
// First-party VM dashboard contribution. Loaded only for a live binding.
let vmRefreshInterval = null;
// ═══ VM ════════════════════════════════════════════════════════════════

let vncRfb = null;
let vncConnectedVm = null;
let vncModule = null;
let clipboardEnabled = false;

function vncLog(msg, level) {
    const log = document.getElementById('vm-vnc-log');
    if (!log) return;
    const time = new Date().toLocaleTimeString();
    const colors = { info: 'var(--text-primary)', warn: '#FFD600', error: '#FF5252', success: '#4CAF50' };
    const div = document.createElement('div');
    div.innerHTML = `<span style="color:var(--text-secondary)">${time}</span> <span style="color:${colors[level] || colors.info}">${escapeHtml(msg)}</span>`;
    log.appendChild(div); log.scrollTop = log.scrollHeight;
}

async function loadVM() {
    if (!vmVisible) return;
    const current = vmGeneration;
    try { await loadVMStatus(); await loadVMActivity(); }
    catch (err) { console.error('Failed to load VM:', err); }
    if (vmVisible && current === vmGeneration) startVMRefreshLoop();
}

function startVMRefreshLoop() {
    if (vmRefreshInterval) clearInterval(vmRefreshInterval);
    vmRefreshInterval = setInterval(() => { loadVMStatus(); }, 5000);
}

async function loadVMStatus() {
    const current = vmGeneration;
    const container = document.getElementById('vm-status-bar');
    try {
        const res = await apiGet('/api/plugins/vm');
        if (!vmVisible || current !== vmGeneration) return;
        if (!res.ok) {
            disconnectVNC();
            PraxisDashboard.load();
            container.innerHTML = '<div class="data-item"><span class="name" style="color:var(--text-secondary)">VM feature unavailable. Install the compatibility preset and set VM_ENABLED=true.</span></div>';
            return;
        }
        const data = await res.json();
        if (!vmVisible || current !== vmGeneration) return;
        const vms = data.vms || [];
        if (data.config && data.config.vm_enabled === false) {
            const setup = data.config.vm_compiled === false
                ? 'This binary has no VM package. Build with the compatibility or vm feature.'
                : 'VM package is disabled or missing. Install the compatibility preset, set VM_ENABLED=true, then restart.';
            container.innerHTML = `<div class="data-item"><span class="name">${escapeHtml(setup)}</span></div>`;
            document.getElementById('vm-config').innerHTML = '';
            disconnectVNC();
            return;
        }

        if (vms.length === 0) {
            container.innerHTML = '<div class="data-item"><span class="name">No VMs running</span></div>';
            disconnectVNC();
        } else {
            container.innerHTML = vms.map(vm => {
                const isStopped = vm.status === 'stopped' || vm.status === 'stopped';
                return `<div class="data-item">
                    <div>
                        <span class="name">${escapeHtml(vm.name)}</span>
                        <span class="meta">Status: ${escapeHtml(vm.status)} | PID: ${vm.pid || '-'} | VNC: ${vm.vnc_port || '-'} | Layout: ${(vm.keyboard_layout || 'us').toUpperCase()}${vm.current_iso ? ' | CD: ' + escapeHtml(vm.current_iso.split('/').pop()) : ''}</span>
                    </div>
                    <div class="actions">
                        ${isStopped ? `<button class="btn btn-sm btn-primary" onclick="vmStartByName('${escapeHtml(vm.name)}')">Start</button>` : ''}
                        ${vm.status === 'running' ? `<button class="btn btn-sm btn-danger" onclick="vmStop('${escapeHtml(vm.name)}')">Stop</button>` : ''}
                        ${vm.status === 'running' ? `<button class="btn btn-sm btn-secondary" onclick="vmReboot('${escapeHtml(vm.name)}')">Reboot</button>` : ''}
                    </div>
                </div>`;
            }).join('');

            const runningVm = vms.find(vm => vm.status === 'running');
            if (runningVm) {
                if (vncConnectedVm !== runningVm.name) connectVNC(runningVm.name);
            } else { disconnectVNC(); }
        }

        const config = data.config || {};
        if (config.vm_enabled !== undefined) {
            document.getElementById('vm-config').innerHTML = `
                <div class="data-item"><span class="name">VM Enabled</span><span class="meta">${config.vm_enabled ? 'Yes' : 'No'}</span></div>
                <div class="data-item"><span class="name">CPU Cores</span><span class="meta">${config.vm_cpu_cores || '-'}</span></div>
                <div class="data-item"><span class="name">RAM (MB)</span><span class="meta">${config.vm_ram_mb || '-'}</span></div>
                <div class="data-item"><span class="name">Disk Size</span><span class="meta">${config.vm_disk_size || '-'}</span></div>
                <div class="data-item"><span class="name">Architecture</span><span class="meta">${config.vm_arch || '-'}</span></div>`;
        }
    } catch (err) { if (!vmVisible || current !== vmGeneration) return; container.innerHTML = `<div class="data-item"><span class="name" style="color:var(--error)">Error: ${escapeHtml(err.message)}</span></div>`; }
}

async function vmApi(path, options = {}) {
    const response = await apiFetch(path, options);
    if (!response.ok) {
        const detail = response.status === 503
            ? 'VM package unavailable. Refresh the page or restart Praxis after checking the worker.'
            : 'Check the VM name, QEMU installation and guest configuration.';
        throw new Error(`VM request failed (${response.status}). ${detail}`);
    }
    return response.json();
}

async function vmStartByName(name) {
    const savedLayout = 'us';

    const layouts = [
        { value: 'us', label: 'US (QWERTY)' }, { value: 'de', label: 'DE (QWERTZ)' },
        { value: 'fr', label: 'FR (AZERTY)' }, { value: 'es', label: 'ES (Spanish)' },
        { value: 'it', label: 'IT (Italian)' }, { value: 'gb', label: 'GB (British)' },
    ];
    const layoutOptions = layouts.map(l => `<option value="${l.value}" ${l.value === savedLayout ? 'selected' : ''}>${l.label}</option>`).join('');

    showModal('Start VM', `
        <div class="form-group"><label>VM Name</label><input type="text" id="vm-start-name" value="${escapeHtml(name)}" style="width:100%;padding:0.5rem"></div>
        <div class="form-group"><label>CPU Cores</label><input type="number" id="vm-start-cpu" value="2" style="width:100%;padding:0.5rem"></div>
        <div class="form-group"><label>RAM (MB)</label><input type="number" id="vm-start-ram" value="4096" style="width:100%;padding:0.5rem"></div>
        <div class="form-group"><label>Disk Size</label><input type="text" id="vm-start-disk" value="40G" style="width:100%;padding:0.5rem"></div>
        <div class="form-group"><label>ISO Path (optional)</label><input type="text" id="vm-start-iso" placeholder="/path/to/linux.iso" style="width:100%;padding:0.5rem"></div>
        <div class="form-group"><label>Keyboard Layout</label><select id="vm-start-layout" style="width:100%;padding:0.5rem">${layoutOptions}</select></div>
        <button class="btn btn-primary" style="margin-top:1rem" onclick="vmDoStart()">Start VM</button>`);
}

async function vmStart() { vmStartByName('praxis-vm'); }

async function vmDoStart() {
    const body = {
        name: document.getElementById('vm-start-name').value || 'praxis-vm',
        cpu_cores: parseInt(document.getElementById('vm-start-cpu').value) || 2,
        ram_mb: parseInt(document.getElementById('vm-start-ram').value) || 4096,
        disk_size: document.getElementById('vm-start-disk').value || '40G',
        keyboard_layout: document.getElementById('vm-start-layout').value || 'us',
    };
    const iso = document.getElementById('vm-start-iso').value;
    if (iso) body.iso_path = iso;
    try {
        const data = await vmApi('/api/plugins/vm/start', { method: 'POST', body: JSON.stringify(body) });
        closeModal();
        alert(data.message || data.error || 'Done');
        loadVM();
    } catch (err) { alert('Failed: ' + err.message); }
}

async function vmStop(name) {
    name = name || 'praxis-vm';
    if (!confirm(`Stop VM "${name}"?`)) return;
    try {
        const data = await vmApi('/api/plugins/vm/stop', { method: 'POST', body: JSON.stringify({ name }) });
        alert(data.message || data.error || 'Stopped');
        loadVM();
    } catch (err) { alert('Failed: ' + err.message); }
}

async function vmReboot(name) {
    name = name || 'praxis-vm';
    if (!confirm('Reboot VM?')) return;
    try {
        const data = await vmApi('/api/plugins/vm/reboot', { method: 'POST', body: JSON.stringify({ name }) });
        alert(data.message || data.error || 'Rebooting');
        loadVM();
    } catch (err) { alert('Failed: ' + err.message); }
}

function vmRefresh() { loadVM(); }

async function vmInsertCD() {
    const iso = prompt('Path to ISO file:');
    if (!iso) return;
    try {
        const data = await vmApi('/api/plugins/vm/cd', { method: 'POST', body: JSON.stringify({ name: 'praxis-vm', iso_path: iso }) });
        alert(data.message || data.error || 'CD inserted');
    } catch (err) { alert('Failed: ' + err.message); }
}

async function vmEjectCD() {
    try {
        const data = await vmApi('/api/plugins/vm/cd', { method: 'POST', body: JSON.stringify({ name: 'praxis-vm', iso_path: null }) });
        alert(data.message || data.error || 'CD ejected');
    } catch (err) { alert('Failed: ' + err.message); }
}

async function vmCreateSnapshot() {
    const name = prompt('Snapshot name:');
    if (!name) return;
    try {
        const data = await vmApi('/api/plugins/vm/snapshot', { method: 'POST', body: JSON.stringify({ snapshot_name: name }) });
        alert(data.message || data.error || 'Snapshot created');
    } catch (err) { alert('Failed: ' + err.message); }
}

async function vmAddSharedFolder() {
    showModal('Add Shared Folder', `
        <div class="form-group"><label>Host Path</label><input type="text" id="vm-sf-host" placeholder="/home/user/projects" style="width:100%;padding:0.5rem"></div>
        <div class="form-group"><label>Mount Point (in VM)</label><input type="text" id="vm-sf-mount" value="/mnt/projects" style="width:100%;padding:0.5rem"></div>
        <button class="btn btn-primary" style="margin-top:1rem" onclick="vmDoAddSharedFolder()">Add</button>`);
}

async function vmDoAddSharedFolder() {
    const host_path = document.getElementById('vm-sf-host').value;
    const mount_point = document.getElementById('vm-sf-mount').value;
    if (!host_path) { alert('Host path required'); return; }
    try {
        const data = await vmApi('/api/plugins/vm/shared-folder', { method: 'POST', body: JSON.stringify({ host_path, mount_point }) });
        closeModal();
        alert(data.message || data.error || 'Shared folder added');
    } catch (err) { alert('Failed: ' + err.message); }
}

async function vmClipboardToggle() {
    clipboardEnabled = !clipboardEnabled;
    const btn = document.getElementById('vm-clipboard-btn');
    if (clipboardEnabled) {
        btn.textContent = '📋 Clipboard: ON';
        btn.style.background = 'rgba(0,217,255,0.2)';
        btn.style.color = 'var(--accent-cyan)';
        btn.style.borderColor = 'var(--accent-cyan)';
        alert('Shared clipboard enabled. You can now copy text from the dashboard to the VM clipboard and vice versa.');
    } else {
        btn.textContent = '📋 Clipboard: OFF';
        btn.style.background = '';
        btn.style.color = '';
        btn.style.borderColor = '';
    }
}

async function vmClipboardSet() {
    const text = prompt('Enter text to copy to VM clipboard:');
    if (!text) return;
    try {
        const res = await apiFetch('/api/plugins/vm/clipboard/set', {
            method: 'POST',
            body: JSON.stringify({ name: 'praxis-vm', content: text })
        });
        const data = await res.json();
        if (data.success) vncLog(`Clipboard set: ${text.substring(0, 30)}${text.length > 30 ? '...' : ''}`, 'success');
        else vncLog(`Clipboard failed: ${data.error}`, 'error');
    } catch (err) { vncLog(`Clipboard error: ${err.message}`, 'error'); }
}

async function vmClipboardGet() {
    try {
        const data = await vmApi('/api/plugins/vm/clipboard/get?name=praxis-vm');
        if (data.success && data.content) {
            await navigator.clipboard.writeText(data.content);
            vncLog(`Clipboard copied to host: ${data.content.substring(0, 30)}${data.content.length > 30 ? '...' : ''}`, 'success');
        } else {
            vncLog(`Clipboard read failed: ${data.error || 'empty'}`, 'error');
        }
    } catch (err) { vncLog(`Clipboard error: ${err.message}`, 'error'); }
}

// VNC
async function connectVNC(vmName) {
    const current = vmGeneration;
    const placeholder = document.getElementById('vm-vnc-placeholder');
    const screen = document.getElementById('vm-vnc-screen');
    const log = document.getElementById('vm-vnc-log');
    if (log) log.innerHTML = '';
    if (!screen) return;

    try {
        vncLog(`Starting VNC to ${vmName}`, 'info');
        if (!vncModule) { vncModule = await import('/plugins/vm/novnc/core/rfb.js'); vncLog('noVNC loaded', 'success'); }
        if (!vmVisible || current !== vmGeneration) return;
        const RFB = vncModule.default || vncModule.RFB;
        if (vncRfb) { vncRfb.disconnect(); vncRfb = null; }
        const wsUrl = `${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}/api/plugins/vm/vnc/ws?vm=${encodeURIComponent(vmName)}&token=${encodeURIComponent(PraxisDashboard.token() || '')}`;

        placeholder.textContent = 'Connecting to VNC...';
        placeholder.style.display = 'block';
        screen.innerHTML = '';

        vncRfb = new RFB(screen, wsUrl, { credentials: { password: '' }, shared: true, wsProtocols: ['binary'] });
        vncRfb.addEventListener('connect', () => {
            vncLog('VNC connected!', 'success');
            placeholder.style.display = 'none';
            vncConnectedVm = vmName;
        });
        vncRfb.addEventListener('disconnect', e => {
            vncLog(`Disconnected: ${e.detail?.reason || 'unknown'}`, e.detail?.clean ? 'warn' : 'error');
            placeholder.style.display = 'block';
            placeholder.textContent = 'VNC disconnected. Click Refresh to reconnect.';
            vncConnectedVm = null;
        });
        vncRfb.scaleViewport = true;
        vncRfb.resizeSession = false;
    } catch (err) { vncLog(`VNC error: ${err.message}`, 'error'); }
}

function disconnectVNC() {
    if (vncRfb) { vncRfb.disconnect(); vncRfb = null; }
    const placeholder = document.getElementById('vm-vnc-placeholder');
    const screen = document.getElementById('vm-vnc-screen');
    if (placeholder) { placeholder.style.display = 'block'; placeholder.textContent = 'VM not running. Click "Start VM" to begin.'; }
    if (screen) screen.innerHTML = '';
    vncConnectedVm = null;
}

// Activity Log
async function loadVMActivity() {
    const container = document.getElementById('vm-activity-log');
    try {
        const res = await apiGet('/api/tool-activity?limit=100');
        if (!res.ok) return;
        const data = await res.json();
        const activities = data.activities || [];
        if (activities.length === 0) {
            container.innerHTML = '<div style="color:var(--text-secondary)">No VM activity yet.</div>'; return;
        }
        container.innerHTML = activities.map(a => {
            const time = a.created_at ? new Date(a.created_at).toLocaleTimeString() : '';
            let color = 'var(--text-primary)'; let icon = '[TOOL]';
            const name = a.action || '';
            if (name === 'vm_shell') { color = '#00D9FF'; icon = '[SHELL]'; }
            else if (name === 'vm_keys') { color = '#FFD600'; icon = '[KEYS]'; }
            else if (name === 'vm_mouse') { color = '#FF9800'; icon = '[MOUSE]'; }
            else if (name === 'vm_screenshot') { color = '#6C63FF'; icon = '[SCREEN]'; }
            else if (name === 'vm_start') { color = '#00E676'; icon = '[START]'; }
            else if (name === 'vm_stop') { color = '#FF5252'; icon = '[STOP]'; }
            else if (name === 'vm_file_transfer') { color = '#E040FB'; icon = '[FILE]'; }
            else if (name === 'vm_snapshot') { color = '#7C4DFF'; icon = '[SNAP]'; }
            else if (name === 'vm_shared_folder') { color = '#00BCD4'; icon = '[SHARE]'; }
            else if (name === 'execute_terminal') { color = '#4CAF50'; icon = '[TERM]'; }
            else if (name === 'write_file') { color = '#2196F3'; icon = '[WRITE]'; }
            else if (name === 'edit_file') { color = '#2196F3'; icon = '[EDIT]'; }
            else if (name === 'read_file') { color = '#90CAF9'; icon = '[READ]'; }
            else if (name.startsWith('agent_')) { color = '#FFA726'; icon = '[AGENT]'; }
            else if (name.startsWith('discord_')) { color = '#7289DA'; icon = '[DISCORD]'; }
            else if (name.startsWith('learn_')) { color = '#CE93D8'; icon = '[LEARN]'; }
            else if (name.startsWith('llm_')) { color = '#FF5252'; icon = '[LLM]'; }

            let inputSummary = '';
            try {
                const inp = JSON.parse(a.input || '{}');
                if (inp.command) inputSummary = inp.command.substring(0, 80);
                else if (inp.keys) inputSummary = `"${inp.keys}"`;
                else if (inp.path) inputSummary = inp.path;
                else if (inp.message) inputSummary = inp.message.substring(0, 60);
                else if (inp.action) inputSummary = inp.action;
                else inputSummary = (a.input || '').substring(0, 80);
            } catch { inputSummary = (a.input || '').substring(0, 80); }
            const outputPreview = (a.output || '').substring(0, 120).replace(/\n/g, ' ');
            const vmTag = a.vm_id ? ` [${escapeHtml(a.vm_id)}]` : '';

            return `<div style="margin-bottom:0.3rem;padding:0.2rem 0;border-bottom:1px solid rgba(255,255,255,0.05)">
                <span style="color:var(--text-secondary);font-size:0.75rem">${time}</span>
                <span style="color:${color};font-weight:600;font-size:0.8rem">${icon}${vmTag} ${escapeHtml(name)}</span>
                <span style="color:var(--text-primary);font-size:0.8rem"> ${escapeHtml(inputSummary)}</span>
                ${outputPreview ? `<div style="color:var(--text-secondary);font-size:0.75rem;padding-left:1rem;white-space:nowrap;overflow:hidden;text-overflow:ellipsis">${escapeHtml(outputPreview)}</div>` : ''}
            </div>`;
        }).join('');
    } catch (err) { console.error('Activity load error:', err); }
}


PraxisDashboard.register('vm', {
    onShow() { vmVisible = true; return loadVM(); },
    onHide() {
        vmVisible = false; vmGeneration++;
        clearInterval(vmRefreshInterval);
        vmRefreshInterval = null;
        disconnectVNC();
    }
});

Object.assign(window, { vmStart, vmStartByName, vmDoStart, vmStop, vmReboot, vmRefresh, vmInsertCD, vmEjectCD, vmCreateSnapshot, vmAddSharedFolder, vmDoAddSharedFolder, vmClipboardToggle, vmClipboardSet, vmClipboardGet });
})();
