// Settings' Voice page (settings.html #pg-voice, entries/voice.md). Loads
// after settings.js and uses its helpers: call, listen, failed, $,
// checkRadios. Every change is saved at once through voice:set, which
// checks it; a refusal (a chord another shortcut has) is said in place.
(() => {
  if (!document.getElementById('pg-voice')) return;
  const ENGINE_NAMES = {
    'whisper-base': 'Whisper base',
    'parakeet-110m': 'Parakeet 110M',
    'whisper-tiny': 'Whisper tiny',
    'whisper-small': 'Whisper small',
    'parakeet-0.6b': 'Parakeet 0.6B',
    gemini: 'Gemini',
  };
  const MODS = { Control: 'Ctrl', Alt: 'Alt', Shift: 'Shift', Meta: 'Super' };
  const ORDER = ['Ctrl', 'Alt', 'Shift', 'Super'];
  let snap = null;
  let capture = null; // { held: Set, best: [] } while recording a chord
  let wakeTimer = 0; // asks again while listening is waiting to start

  const v = {
    dictation: $('voice-dictation'),
    hotkey: $('voice-hotkey'),
    change: $('voice-hotkey-change'),
    warn: $('voice-hotkey-warn'),
    row: $('voice-hotkey-row'),
    language: $('voice-language'),
    mic: $('voice-mic'),
    cleanup: $('voice-cleanup'),
    cleanupNote: $('voice-cleanup-note'),
    progress: $('voice-progress'),
    download: $('voice-download'),
    status: $('voice-engine-status'),
    wake: $('voice-wake'),
    wakeWord: $('voice-wake-word'),
    wakeSave: $('voice-wake-save'),
    wakeStatus: $('voice-wake-status'),
  };

  const label = (acc) => String(acc || '').replace(/Super/g, 'Win');
  const speak = (t) => window.AevalA11y && window.AevalA11y.say(t);
  const mb = (b) => `${Math.round(b / 1e6)} MB`;

  function status(text, error) {
    v.status.hidden = !text;
    v.status.textContent = text || '';
    v.status.classList.toggle('error', !!error);
  }

  function warn(text) {
    v.warn.hidden = !text;
    v.warn.textContent = text || '';
  }

  function option(value, text, selected) {
    const o = document.createElement('option');
    o.value = value;
    o.textContent = text;
    o.selected = selected;
    return o;
  }

  function engineOf(id) {
    return (snap && snap.engines.find((e) => e.id === id)) || null;
  }

  function render() {
    if (!snap) return;
    const s = snap.settings;
    v.dictation.checked = s.dictation;
    v.hotkey.textContent = capture ? 'Press keys…' : snap.hotkeyLabel;
    v.row.classList.toggle('capturing', !!capture);
    v.change.textContent = capture ? 'Cancel' : 'Change';
    v.change.disabled = !s.dictation;
    $('voice-dictation-summary').textContent = s.dictation ? snap.hotkeyLabel : 'Off';

    checkRadios('[data-voice-engine]', (r) => r.dataset.voiceEngine === s.engine);
    for (const r of document.querySelectorAll('[data-voice-engine]')) {
      const e = engineOf(r.dataset.voiceEngine);
      r.disabled = !!e && !e.available;
      const note = r.querySelector('.choice-note');
      if (!note || !e) continue;
      if (!note.dataset.base) note.dataset.base = note.textContent;
      let extra = '';
      if (e.local) extra = e.ready ? ' Downloaded.' : ` Downloads ${mb(e.bytes)} the first time.`;
      else if (!e.available) extra = ' Add a Gemini key under Providers first.';
      // A Parakeet engine asked for a language it does not hear.
      if (e.standIn) extra = ' For the language chosen, Whisper base is used.' + extra;
      note.textContent = note.dataset.base + extra;
    }
    const chosen = engineOf(s.engine);
    $('voice-engine-summary').textContent = ENGINE_NAMES[s.engine] || s.engine;
    v.download.hidden = !chosen || !chosen.local || chosen.ready || !!snap.download;
    if (!snap.cpuOk && chosen && chosen.local) {
      status('This PC lacks the processor features voice on this PC needs (AVX2). Choose Gemini instead.', true);
    }

    v.language.replaceChildren(...snap.languages.map((l) => option(l.id, l.name, l.id === s.language)));
    const lang = snap.languages.find((l) => l.id === s.language);
    $('voice-language-summary').textContent = lang ? lang.name : s.language;

    const devices = snap.devices || [];
    v.mic.replaceChildren(
      option('', devices.length ? `Windows default (${devices[0]})` : 'Windows default', !s.device),
      ...devices.map((d) => option(d, d, d === s.device)),
    );
    v.mic.disabled = !devices.length;
    $('voice-mic-summary').textContent = s.device || (devices.length ? 'Windows default' : 'No microphone found');

    v.cleanup.checked = s.cleanup;
    $('voice-cleanup-summary').textContent = s.cleanup ? 'On' : 'Off';
    const budget = `${((snap.cleanupBudgetMs || 800) / 1000).toFixed(1)} s`;
    const cost = s.cleanup
      ? `Each dictation waits up to ${budget} longer for it.`
      : `Off, your words go in the moment they are heard, with "um" and "uh" taken out. On, each dictation waits up to ${budget} longer.`;
    v.cleanupNote.textContent = snap.cleanupModel
      ? `${cost} Uses ${snap.cleanupModel}, a free model, and never adds words. If it is slower than that, you get the words as heard.`
      : `${cost} Needs a free OpenRouter model, set up under Providers. Until then you get the words as heard.`;
    renderWake(s);
    const t = snap.lastTimings;
    const last = $('voice-last-timing');
    if (last) {
      last.hidden = !t;
      if (t) last.textContent = `Your last dictation: text in ${(t.totalMs / 1000).toFixed(1)} s after you let go.`;
    }
  }

  // Listening for the name: the switch, the name, and how it stands.
  function renderWake(s) {
    const word = s.wakeWord || 'Aevaleron';
    const w = snap.wake || { state: 'off' };
    v.wake.checked = !!s.wake;
    $('voice-wake-label').textContent = `Start when I say ${word}`;
    if (document.activeElement !== v.wakeWord) v.wakeWord.value = word;
    let line = '';
    let error = false;
    if (s.wake && w.state === 'listening') line = `Listening for "${word}".`;
    else if (s.wake && w.state === 'waiting') line = w.message || 'Starting…';
    else if (s.wake && w.state === 'error') {
      line = w.message || 'It cannot listen right now.';
      error = true;
    } else if (s.wake) line = 'Starting…';
    $('voice-wake-summary').textContent = s.wake ? word : 'Off';
    wakeStatus(line, error);
    clearTimeout(wakeTimer);
    if (s.wake && w.state !== 'listening') wakeTimer = setTimeout(() => !capture && load(), 3000);
  }

  function wakeStatus(text, error) {
    v.wakeStatus.hidden = !text;
    v.wakeStatus.textContent = text || '';
    v.wakeStatus.classList.toggle('error', !!error);
  }

  function saveWord() {
    const word = v.wakeWord.value.trim();
    if (!snap || !word || word === snap.settings.wakeWord) return;
    call('voice:set', { wakeWord: word }).then((r) => {
      if (failed(r) || !r) return;
      if (!r.ok) {
        wakeStatus(r.error, true);
        speak(r.error);
        return;
      }
      speak(`Listening for ${word}.`);
      v.wakeWord.blur();
      load();
    });
  }

  function load() {
    return call('voice:get').then((r) => {
      if (failed(r) || !r) return;
      snap = r;
      render();
    });
  }

  function save(patch) {
    return call('voice:set', patch).then((r) => {
      if (failed(r) || !r) return false;
      if (!r.ok) {
        warn(r.error);
        speak(r.error);
        render();
        return false;
      }
      warn('');
      return load().then(() => true);
    });
  }

  // ------------------------------------------------------ chord capture

  function startCapture() {
    capture = { held: new Set(), best: [], key: null };
    warn('');
    call('voice:capture-hotkey', { on: true });
    render();
    v.change.focus();
    speak('Hold the new keys, then let go. Esc cancels.');
  }

  function endCapture() {
    capture = null;
    call('voice:capture-hotkey', { on: false });
    render();
  }

  function chordName(mods, key) {
    const parts = ORDER.filter((m) => mods.includes(m));
    if (key) parts.push(key);
    return parts.join('+');
  }

  function keyName(e) {
    if (/^Key[A-Z]$/.test(e.code)) return e.code.slice(3);
    if (/^Digit\d$/.test(e.code)) return e.code.slice(5);
    if (/^F\d{1,2}$/.test(e.key)) return e.key;
    if (e.code === 'Space') return 'Space';
    return null;
  }

  document.addEventListener('keydown', (e) => {
    if (!capture) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.key === 'Escape' && !capture.held.size) {
      endCapture();
      speak('Kept the old shortcut.');
      return;
    }
    const mod = MODS[e.key];
    if (mod) capture.held.add(mod);
    else {
      const k = keyName(e);
      if (k) capture.key = k;
    }
    const held = [...capture.held];
    if (held.length + (capture.key ? 1 : 0) > capture.best.length + (capture.bestKey ? 1 : 0)) {
      capture.best = held;
      capture.bestKey = capture.key;
    }
    v.hotkey.textContent = label(chordName(held, capture.key)) || 'Press keys…';
  }, true);

  document.addEventListener('keyup', (e) => {
    if (!capture) return;
    e.preventDefault();
    const mod = MODS[e.key];
    if (mod) capture.held.delete(mod);
    else capture.key = null;
    if (capture.held.size || capture.key) return;
    // Everything is up: the most keys held at once is the chord.
    const acc = chordName(capture.best, capture.bestKey);
    endCapture();
    if (!acc) return;
    save({ hotkey: acc }).then((ok) => {
      if (ok) speak(`Hold ${label(acc)} to talk.`);
    });
  }, true);

  v.change.addEventListener('click', () => (capture ? endCapture() : startCapture()));
  v.dictation.addEventListener('change', () => save({ dictation: v.dictation.checked }));
  v.cleanup.addEventListener('change', () => save({ cleanup: v.cleanup.checked }));
  v.wake.addEventListener('change', () => save({ wake: v.wake.checked }));
  v.wakeSave.addEventListener('click', saveWord);
  v.wakeWord.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') saveWord();
  });
  v.language.addEventListener('change', () => save({ language: v.language.value }));
  v.mic.addEventListener('change', () => save({ device: v.mic.value }));
  for (const r of document.querySelectorAll('[data-voice-engine]')) {
    r.addEventListener('click', () => {
      if (snap && snap.settings.engine !== r.dataset.voiceEngine) save({ engine: r.dataset.voiceEngine });
    });
  }

  v.download.addEventListener('click', () => {
    v.download.hidden = true;
    v.progress.hidden = false;
    v.progress.firstElementChild.style.transform = 'scaleX(0)';
    status('Downloading…');
    call('voice:download').then((r) => {
      v.progress.hidden = true;
      if (failed(r) || !r) return;
      if (r.ok) {
        status('Ready. Voice works offline now.');
        speak('Voice model downloaded.');
      } else {
        status(r.error, true);
        speak(r.error);
      }
      load();
    });
  });

  listen('voice:progress', (p) => {
    if (!p || !p.total) return;
    v.progress.hidden = false;
    v.progress.firstElementChild.style.transform = `scaleX(${(p.got / p.total).toFixed(3)})`;
    status(`Downloading… ${Math.round((p.got * 100) / p.total)}%`);
  });

  window.addEventListener('focus', () => {
    if (!capture) load();
  });

  load();
})();
