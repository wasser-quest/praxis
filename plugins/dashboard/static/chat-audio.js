// Dashboard reply audio. The server owns the durable bytes; the browser keeps
// only metadata and one object URL. Replay never calls a TTS provider.
const chatTtsClips = new Map();
// Entry identity is also a revision: stale HTTP reads cannot undo a newer
// settings event or a local toggle (including an ON → OFF → ON transition).
const chatTtsSettings = new Map();
let chatTtsQueue = [];
let chatTtsCurrent = null;
let chatTtsState = 'idle';
let chatTtsGeneration = 0;
let chatTtsObjectUrl = null;
let chatTtsUnlocked = false;

function chatTtsKey(userId, messageId) {
    return JSON.stringify([userId, String(messageId)]);
}

function chatReceiveTts(payload, userId = chatUserId, autoplay = true) {
    if (!payload) return;
    const messageId = Number.isSafeInteger(payload.message_id) && payload.message_id > 0
        ? payload.message_id : null;
    const dataUrl = typeof payload.audio === 'string' && payload.audio.startsWith('data:audio/')
        ? payload.audio : null;
    if (!messageId && !dataUrl) return;
    const key = chatTtsKey(userId, messageId || dataUrl);
    let clip = chatTtsClips.get(key);
    const isNew = !clip;
    if (!clip) {
        clip = { key, userId, messageId, dataUrl };
        chatTtsClips.set(key, clip);
    }

    if (messageId && userId === chatUserId) {
        const div = [...document.querySelectorAll('#chat-messages .chat-msg.assistant')]
            .find(el => el.dataset.messageId === String(messageId));
        if (div) chatAttachTtsButton(div, clip);
        // If assistant_saved was lost, recover the reply identity from history.
        else if (isNew && autoplay) pollChatMessages();
    } else if (isNew) {
        // Transient feedback / Discord side-channel audio still gets a replay
        // button, without attaching it to an unrelated active-session reply.
        const div = addChatMessage('system', userId === chatUserId ? 'Audioantwort' : '🎮 Audio aus Discord');
        chatAttachTtsButton(div, clip);
    }
    // Register even when OFF or loading old history: manual replay always works.
    // Live SSE and reconnect/history may report the same clip; play it only once.
    if (isNew && autoplay && chatTtsOn) {
        chatTtsQueue.push(clip);
        chatTtsPlayNext();
    }
}

function chatAttachTtsButton(div, clip) {
    if (!div || !clip || div.querySelector('.chat-audio-controls')) return;
    const controls = document.createElement('div');
    controls.className = 'chat-audio-controls';
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'btn btn-sm btn-secondary chat-audio-play';
    button.dataset.audioKey = clip.key;
    button.onclick = () => chatPlayTts(clip, true);
    const status = document.createElement('span');
    status.className = 'chat-audio-status';
    status.setAttribute('role', 'status');
    controls.appendChild(button);
    controls.appendChild(status);
    (div.querySelector('.msg-content') || div).appendChild(controls);
    chatTtsUpdateUI();
}

function chatTtsPlayer() {
    // Autoplay permission can be per media element (notably on Safari).
    // Never discard the element that was unlocked by the user's gesture.
    if (!chatTtsAudio) chatTtsAudio = new Audio();
    return chatTtsAudio;
}

function chatTtsUnlock() {
    if (chatTtsUnlocked || chatTtsCurrent) return;
    const audio = chatTtsPlayer();
    // A valid 50 ms, mono PCM WAV, not the old truncated MP3. Silence is not
    // muted: muted playback does not grant permission for later audible audio.
    const bytes = new Uint8Array(844);
    const view = new DataView(bytes.buffer);
    for (const [offset, text] of [[0, 'RIFF'], [8, 'WAVE'], [12, 'fmt '], [36, 'data']]) {
        for (let i = 0; i < text.length; i++) bytes[offset + i] = text.charCodeAt(i);
    }
    view.setUint32(4, 836, true);
    view.setUint32(16, 16, true);
    view.setUint16(20, 1, true);
    view.setUint16(22, 1, true);
    view.setUint32(24, 8000, true);
    view.setUint32(28, 16000, true);
    view.setUint16(32, 2, true);
    view.setUint16(34, 16, true);
    view.setUint32(40, 800, true);
    audio.src = 'data:audio/wav;base64,' + btoa(String.fromCharCode(...bytes));
    audio.volume = 1;
    audio.play().then(() => { chatTtsUnlocked = true; }).catch(() => {
        // Best effort only. Actual playback reports a visible manual fallback.
    });
}

function chatApplyTtsSetting(userId, on) {
    const previous = chatTtsSettings.get(userId);
    chatTtsSettings.set(userId, { enabled: on });
    if (userId === chatUserId) {
        const wasOn = chatTtsOn;
        chatTtsOn = on;
        const btn = document.getElementById('chat-tts-btn');
        if (btn) {
            btn.setAttribute('aria-pressed', String(on));
            btn.classList.toggle('tts-on', on);
            btn.textContent = on ? '🔊' : '🔉';
        }
        // Repeated OFF settings (e.g. an unrelated context save) must not
        // interrupt a manual replay started while automatic speech was OFF.
        if (!on && wasOn) chatTtsActive(false);
    } else if (!on) {
        chatTtsQueue = chatTtsQueue.filter(clip => clip.userId !== userId);
        if (previous?.enabled && chatTtsCurrent?.userId === userId) {
            chatTtsStopCurrent();
            chatTtsUpdateUI();
            chatTtsPlayNext();
        }
    }
}

function chatReceiveTtsSetting(event, userId) {
    try {
        const d = JSON.parse(event.data);
        const payload = typeof d.data === 'string' ? JSON.parse(d.data) : (d.data || d);
        if (typeof payload.enabled === 'boolean') chatApplyTtsSetting(userId, payload.enabled);
    } catch (err) { console.warn('[TTS] invalid settings event:', err); }
}

async function chatToggleTts() {
    const uid = chatUserId;
    const on = !chatTtsOn;
    chatApplyTtsSetting(uid, on);
    if (on) chatTtsUnlock();
    try {
        const res = await apiFetch(`/api/contexts/${encodeURIComponent(uid)}`, {
            method: 'PUT', body: JSON.stringify({ settings: { web_chat_tts: on } })
        });
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        if (uid === chatUserId) addChatMessage('system', on ? '🔊 Replies will be read aloud.' : '🔉 TTS off. Saved audio can still be played.');
    } catch (err) {
        if (uid === chatUserId) {
            await loadTtsSwitchState();
            addChatMessage('feedback', 'Could not save TTS setting: ' + err.message);
        }
    }
}

function chatTtsPlayNext() {
    if (chatTtsCurrent || !chatTtsQueue.length || !chatTtsOn) return;
    chatPlayTts(chatTtsQueue.shift());
}

async function chatPlayTts(clip, manual = false) {
    if (!clip || (!manual && !chatTtsOn)) return;
    if (manual) {
        chatTtsQueue = [];
        if (chatTtsCurrent?.key === clip.key && ['playing', 'loading'].includes(chatTtsState)) {
            chatTtsActive(false);
            return;
        }
    }
    const retryReady = chatTtsCurrent?.key === clip.key && chatTtsState === 'ready';
    if (!retryReady) {
        chatTtsStopCurrent();
        if (manual) chatTtsUnlock(); // synchronous gesture, before fetching bytes
    }
    const generation = ++chatTtsGeneration;
    const audio = chatTtsPlayer();
    chatTtsCurrent = clip;
    chatTtsState = 'loading';
    chatTtsUpdateUI();
    try {
        if (!manual) {
            // SSE can be delayed or lost. Recheck the active dashboard AND the
            // source context before fetching/playing each automatic clip.
            const allowed = await loadTtsSwitchState();
            if (generation !== chatTtsGeneration) return;
            const sourceAllowed = clip.userId === chatUserId ? allowed
                : await loadTtsSwitchState(clip.userId);
            if (generation !== chatTtsGeneration) return;
            if (!allowed || !sourceAllowed || !chatTtsOn) {
                chatTtsStopCurrent();
                chatTtsUpdateUI();
                chatTtsPlayNext();
                return;
            }
        }
        if (!retryReady) {
            let src = clip.dataUrl;
            if (!src) {
                // Construct our own same-origin path; never fetch an SSE-supplied
                // external URL with the dashboard Authorization header.
                const res = await apiGet(`/api/chat/audio/${encodeURIComponent(clip.userId)}/${clip.messageId}`);
                if (!res.ok) throw new Error(`Audio HTTP ${res.status}`);
                const blob = await res.blob();
                if (generation !== chatTtsGeneration) return;
                src = chatTtsObjectUrl = URL.createObjectURL(blob);
            }
            if (generation !== chatTtsGeneration) return;
            audio.src = src;
        }
        audio.onended = () => {
            if (generation !== chatTtsGeneration) return;
            chatTtsStopCurrent();
            chatTtsUpdateUI();
            chatTtsPlayNext();
        };
        audio.onerror = () => {
            if (generation !== chatTtsGeneration) return;
            chatTtsState = 'error';
            chatTtsUpdateUI();
        };
        audio.volume = 1;
        // For a blocked, already-loaded clip this runs directly in the click
        // handler, with no intervening await or second audio element.
        await audio.play();
        if (generation !== chatTtsGeneration) return;
        chatTtsUnlocked = true;
        chatTtsState = 'playing';
        chatTtsUpdateUI();
    } catch (err) {
        if (generation !== chatTtsGeneration) return;
        chatTtsState = err.name === 'NotAllowedError' ? 'ready' : 'error';
        console.warn('[TTS] playback failed:', err.name, err.message);
        chatTtsUpdateUI();
    }
}

function chatTtsStopCurrent() {
    chatTtsGeneration++; // invalidate pending fetches and stale media callbacks
    if (chatTtsAudio) {
        chatTtsAudio.onended = null;
        chatTtsAudio.onerror = null;
        chatTtsAudio.pause();
        chatTtsAudio.removeAttribute('src');
        chatTtsAudio.load();
    }
    if (chatTtsObjectUrl) URL.revokeObjectURL(chatTtsObjectUrl);
    chatTtsObjectUrl = null;
    chatTtsCurrent = null;
    chatTtsState = 'idle';
}

function chatTtsActive(on) {
    if (!on) {
        chatTtsQueue = [];
        chatTtsStopCurrent();
    }
    chatTtsUpdateUI();
}

function chatTtsUpdateUI() {
    const active = chatTtsCurrent;
    const playing = chatTtsState === 'playing';
    const loading = chatTtsState === 'loading';
    const hint = chatTtsState === 'ready' ? 'Autoplay blockiert – zum Abspielen klicken.'
        : chatTtsState === 'error' ? 'Audio konnte nicht abgespielt werden – erneut versuchen.' : '';
    for (const button of document.querySelectorAll('.chat-audio-play')) {
        const current = active && button.dataset.audioKey === active.key;
        button.textContent = current && playing ? '■ Audio stoppen'
            : current && loading ? '■ Laden abbrechen' : '▶ Audio abspielen';
        button.setAttribute('aria-pressed', String(!!(current && (playing || loading))));
        button.parentElement.querySelector('.chat-audio-status').textContent = current ? hint : '';
    }
    const chip = document.getElementById('chat-tts-indicator');
    if (chip) {
        chip.style.display = active ? '' : 'none';
        chip.textContent = playing ? '🔊 Praxis spricht … ■ Stop'
            : loading ? '🔊 Audio lädt … ■ Abbrechen' : '▶ Audio abspielen';
        chip.title = hint || chip.textContent;
        chip.onclick = () => {
            if (chatTtsCurrent && !['playing', 'loading'].includes(chatTtsState)) chatPlayTts(chatTtsCurrent, true);
            else chatTtsActive(false);
        };
    }
    document.getElementById('chat-tts-btn')?.classList.toggle('tts-speaking', playing);
}

// Remote/local Vosk requires PCM WAV, not a MediaRecorder WebM/Opus container.
// Decode in the browser; no local ffmpeg or extra GPU service is required.
function chatEncodeMonoWav(samples, sampleRate) {
    if (!Number.isInteger(sampleRate) || sampleRate < 8000 || sampleRate > 48000 ||
        !samples.length || samples.length > sampleRate * 300 || samples.length * 2 + 44 > 16 * 1024 * 1024) {
        throw new Error('Recording must be nonempty and at most five minutes / 16 MiB.');
    }
    const bytes = new Uint8Array(44 + samples.length * 2);
    const view = new DataView(bytes.buffer);
    for (const [offset, text] of [[0, 'RIFF'], [8, 'WAVE'], [12, 'fmt '], [36, 'data']]) {
        for (let i = 0; i < text.length; i++) bytes[offset + i] = text.charCodeAt(i);
    }
    view.setUint32(4, bytes.length - 8, true);
    view.setUint32(16, 16, true);
    view.setUint16(20, 1, true);
    view.setUint16(22, 1, true);
    view.setUint32(24, sampleRate, true);
    view.setUint32(28, sampleRate * 2, true);
    view.setUint16(32, 2, true);
    view.setUint16(34, 16, true);
    view.setUint32(40, samples.length * 2, true);
    for (let i = 0; i < samples.length; i++) {
        if (!Number.isFinite(samples[i])) throw new Error('Invalid recording samples.');
        const sample = Math.max(-1, Math.min(1, samples[i]));
        view.setInt16(44 + i * 2, Math.round(sample * (sample < 0 ? 32768 : 32767)), true);
    }
    return new Blob([bytes], {type: 'audio/wav'});
}

async function chatPrepareSttAudio(blob, settings) {
    const mime = blob.type.split(';')[0];
    const extension = {'audio/ogg': 'ogg', 'audio/mp4': 'm4a', 'audio/wav': 'wav'}[mime] || 'webm';
    if (settings.voice_stt_type !== 'vosk') return {blob, filename: `speech.${extension}`};
    if (!blob.size || blob.size > 16 * 1024 * 1024) throw new Error('Recording exceeds 16 MiB or is empty.');
    const DecodeContext = globalThis.AudioContext || globalThis.webkitAudioContext;
    const RenderContext = globalThis.OfflineAudioContext || globalThis.webkitOfflineAudioContext;
    if (!DecodeContext || !RenderContext) throw new Error('This browser cannot convert recordings to Vosk PCM WAV.');
    const context = new DecodeContext();
    let decoded;
    try {
        decoded = await context.decodeAudioData(await blob.arrayBuffer());
        // A small timer/codec overrun is trimmed; longer recordings are rejected.
        // 60s at 16kHz/PCM16 fits the unchanged 2 MiB dashboard request limit.
        if (!Number.isFinite(decoded.duration) || decoded.duration <= 0 || decoded.duration > 61 || ![1, 2].includes(decoded.numberOfChannels)) {
            throw new Error('Use a mono/stereo Vosk browser recording of at most 60 seconds.');
        }
    } finally {
        await context.close().catch(() => {});
    }
    const renderer = new RenderContext(1, Math.ceil(Math.min(decoded.duration, 60) * 16000), 16000);
    const source = renderer.createBufferSource();
    source.buffer = decoded;
    source.connect(renderer.destination);
    source.start();
    const rendered = await renderer.startRendering();
    return {blob: chatEncodeMonoWav(rendered.getChannelData(0), 16000), filename: 'speech.wav'};
}
