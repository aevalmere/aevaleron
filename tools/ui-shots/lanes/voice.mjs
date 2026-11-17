// The voice lane's ui-shots scenes, mocks, checks and CSS lint list: the
// bar's mic, the dictation pill and Settings' Voice page. The shape is
// documented at the top of tools/ui-shots/shots.mjs; shared fixtures are in
// tools/ui-shots/kit.mjs.
import { BAR, openBar } from '../kit.mjs';

const SNAPSHOT = {
  settings: { dictation: true, hotkey: 'Ctrl+Super', cleanup: false, language: 'en', engine: 'whisper-base', device: '', wake: false, wakeWord: 'Aevaleron' },
  hotkeyLabel: 'Ctrl+Win',
  devices: ['Microphone (Realtek Audio)', 'Headset (USB Audio)'],
  engines: [
    { id: 'whisper-base', local: true, bytes: 63709472, ready: false, standIn: false, available: true },
    { id: 'parakeet-110m', local: true, bytes: 101335520, ready: false, standIn: false, available: true },
    { id: 'whisper-tiny', local: true, bytes: 44135072, ready: true, standIn: false, available: true },
    { id: 'whisper-small', local: true, bytes: 193672256, ready: false, standIn: false, available: true },
    { id: 'parakeet-0.6b', local: true, bytes: 548946272, ready: false, standIn: false, available: true },
    { id: 'gemini', local: false, bytes: null, ready: false, available: false },
  ],
  languages: [
    { id: 'en', name: 'English' },
    { id: 'auto', name: 'Detect automatically' },
    { id: 'fr', name: 'French' },
  ],
  download: null,
  cleanupModel: 'meta-llama/llama-3.3-70b-instruct:free',
  cleanupBudgetMs: 800,
  lastTimings: { audioS: 7.9, earlyS: 4.1, pieces: 1, earlyMs: 310, stopMs: 4, waitMs: 0, transcribeMs: 290, cleanupMs: 0, insertMs: 6, totalMs: 300 },
  gemini: false,
  cpuOk: true,
  wake: { state: 'off' },
};

export const mocks = {
  'voice:get': SNAPSHOT,
  'voice:set': { ok: true },
  'voice:start': { ok: true },
};

export const cssLintEnforce = ['renderer/voice.css', 'renderer/voice-pill.css'];

export const checks = [
  // Dictation is never focusable and never asks Rust for anything: the pill
  // page only listens.
  {
    name: 'voicePillOnlyListens',
    run: (ctx) => !/aeval\.invoke/.test(ctx.read('renderer/voice-pill.js')) || 'voice-pill.js invokes a channel',
  },
];

const overlay = { name: 'overlay', src: '/renderer/index.html', x: BAR.x, y: BAR.y, w: BAR.w, h: BAR.h, z: 2 };
const BAR_CLIP = { x: BAR.x - 20, y: 0, width: BAR.w + 40, height: 140 };
const PILL = { x: 700, y: 900, w: 200, h: 52 };
const PILL_CLIP = { x: PILL.x - 40, y: PILL.y - 24, width: PILL.w + 80, height: PILL.h + 48 };
const S = { x: 120, y: 130, w: 960, h: 700 };

const mic = (f) =>
  f.evaluate(() => {
    const b = document.getElementById('mic-btn');
    const field = document.getElementById('query-input');
    return { state: b.dataset.state, pressed: b.getAttribute('aria-pressed'), label: b.getAttribute('aria-label'), value: field.value, placeholder: field.placeholder };
  });

export const scenes = [
  // The mic at rest, listening with its meter, and an error in the field.
  {
    name: 'voice-bar',
    backdrop: 'wallpaper',
    windows: [overlay],
    async run(t) {
      const f = t.frame('overlay');
      await openBar(f);
      await t.settle(300);
      await t.shot('voice-01-bar-idle', BAR_CLIP);
      // A keyboard press of the mic: a click without a pointer.
      await f.evaluate(() => document.getElementById('mic-btn').click());
      t.check('voiceMicStarts', (await t.calls('overlay', 'voice:start')).length === 1, 'no voice:start');
      await t.emit('overlay', 'voice:state', { state: 'listening', origin: 'bar' });
      await t.emit('overlay', 'voice:level', { origin: 'bar', level: 0.8 });
      await t.settle(200);
      const on = await mic(f);
      t.check('voiceMicListening', on.state === 'listening' && on.pressed === 'true', JSON.stringify(on));
      await t.shot('voice-02-bar-listening', BAR_CLIP);
      await f.evaluate(() => document.getElementById('mic-btn').click());
      t.check('voiceMicStops', (await t.calls('overlay', 'voice:stop')).length === 1, 'no voice:stop');
      await t.emit('overlay', 'voice:state', { state: 'done', origin: 'bar', text: 'Meet at four pm tomorrow to go over the budget.' });
      await t.settle(200);
      const done = await mic(f);
      t.check('voiceMicFillsField', done.value.startsWith('Meet at four') && done.state === 'idle', JSON.stringify(done));
      await t.shot('voice-03-bar-done', BAR_CLIP);
      await f.evaluate(() => {
        const q = document.getElementById('query-input');
        q.value = '';
        q.dispatchEvent(new Event('input', { bubbles: true }));
      });
      await t.emit('overlay', 'voice:state', { state: 'error', origin: 'bar', message: 'No microphone found. Plug one in, or pick another in Settings.' });
      await t.settle(200);
      const err = await mic(f);
      t.check('voiceMicError', err.placeholder.startsWith('No microphone'), JSON.stringify(err));
      await t.shot('voice-04-bar-error', BAR_CLIP);
      // Esc while listening stops without keeping anything.
      await f.evaluate(() => document.getElementById('mic-btn').click());
      await t.emit('overlay', 'voice:state', { state: 'listening', origin: 'bar' });
      await f.evaluate(() => window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })));
      t.check('voiceEscCancels', (await t.calls('overlay', 'voice:cancel')).length === 1 && (await mic(f)).state === 'idle', 'Esc did not cancel');
    },
  },

  // The dictation pill over a busy desktop: listening, writing, an error.
  {
    name: 'voice-pill',
    backdrop: 'photo-busy',
    windows: [{ name: 'pill', src: '/renderer/voice-pill.html', ...PILL, z: 1 }],
    async run(t) {
      const levels = [0.1, 0.3, 0.7, 0.9, 0.5, 0.8, 0.6, 0.4, 0.9, 0.7, 0.3, 0.5, 0.2];
      await t.emit('pill', 'voice:state', { state: 'listening', origin: 'dictation' });
      for (const level of levels) await t.emit('pill', 'voice:level', { origin: 'dictation', level });
      await t.settle(300);
      await t.shot('voice-05-pill-listening', PILL_CLIP);
      await t.emit('pill', 'voice:state', { state: 'working', origin: 'dictation' });
      await t.settle(200);
      await t.shot('voice-06-pill-writing', PILL_CLIP);
      await t.emit('pill', 'voice:state', { state: 'error', origin: 'dictation', message: "I didn't hear anything. Hold the key and speak." });
      await t.settle(200);
      const said = await t.frame('pill').evaluate(() => document.getElementById('say').textContent);
      t.check('voicePillSaysWhy', said.startsWith("I didn't hear"), said);
      await t.shot('voice-07-pill-error', PILL_CLIP);
    },
  },

  // Settings, Voice: the engine list with its download sizes.
  {
    name: 'voice-settings',
    backdrop: 'wallpaper',
    textScale: true,
    windows: [{ name: 'settings', src: '/settings/settings.html', ...S, z: 1 }],
    async run(t) {
      const st = t.frame('settings');
      await t.settle(300);
      await st.evaluate(() => {
        showPage('pg-voice');
        openSection('sec-voice-engine');
      });
      await t.settle(600);
      await t.shot('voice-08-settings-engine', { x: S.x - 10, y: S.y - 10, width: S.w + 20, height: S.h + 20 });
      const page = await st.evaluate(() => ({
        checked: document.querySelector('[data-voice-engine][aria-checked="true"]')?.dataset.voiceEngine,
        gemini: document.querySelector('[data-voice-engine="gemini"]').disabled,
        summary: document.getElementById('voice-dictation-summary').textContent,
        privacy: document.getElementById('sec-voice-privacy').textContent,
        download: !document.getElementById('voice-download').hidden,
        engine: document.getElementById('voice-engine-summary').textContent,
        credits: document.getElementById('sec-voice-credits').textContent,
        notes: [...document.querySelectorAll('[data-voice-engine] .choice-note')].map((n) => n.textContent),
      }));
      t.check(
        'voiceSettingsPage',
        page.checked === 'whisper-base' && page.gemini && page.summary === 'Ctrl+Win' && page.download && /Nothing is recorded or kept/.test(page.privacy),
        JSON.stringify(page),
      );
      // Six engines, one line each, Whisper base (the default) first; only
      // Parakeet 110M says it is the fastest, and the Parakeet models'
      // licence is named.
      const fastest = page.notes.filter((n) => /fastest/i.test(n));
      t.check(
        'voiceSettingsEngines',
        page.engine === 'Whisper base' && /default/.test(page.notes[0]) && page.notes.length === 6 && fastest.length === 1 && /^On this PC\. Fastest\./.test(fastest[0]) && /NVIDIA.*CC BY 4\.0/.test(page.credits),
        JSON.stringify(page),
      );
      await st.click('[data-voice-engine="whisper-tiny"]');
      await t.settle(200);
      const set = (await t.calls('settings', 'voice:set')).map((c) => c.payload);
      t.check('voiceSettingsSaves', set.some((p) => p.engine === 'whisper-tiny'), JSON.stringify(set));

      // wake lane: listening for the name, off, then on and listening.
      await st.evaluate(() => openSection('sec-voice-wake'));
      await t.settle(400);
      await t.shot('voice-09-settings-wake', { x: S.x - 10, y: S.y - 10, width: S.w + 20, height: S.h + 20 });
      const wake = await st.evaluate(() => ({
        on: document.getElementById('voice-wake').checked,
        summary: document.getElementById('voice-wake-summary').textContent,
        label: document.getElementById('voice-wake-label').textContent,
        word: document.getElementById('voice-wake-word').value,
        note: document.getElementById('sec-voice-wake').textContent,
      }));
      t.check(
        'voiceWakeOffByDefault',
        !wake.on && wake.summary === 'Off' && wake.label === 'Start when I say Aevaleron' && wake.word === 'Aevaleron',
        JSON.stringify(wake),
      );
      // The page says plainly that the microphone stays open and nothing is
      // saved or sent while it waits.
      t.check(
        'voiceWakeSaysMicOpen',
        /microphone stays open/.test(wake.note) && /Nothing is saved or sent while it waits/.test(wake.note) && /red dot/.test(wake.note),
        wake.note,
      );
      await st.evaluate(() => {
        const sw = document.getElementById('voice-wake');
        sw.checked = true;
        sw.dispatchEvent(new Event('change', { bubbles: true }));
        const f = document.getElementById('voice-wake-word');
        f.value = 'Jarvis';
        document.getElementById('voice-wake-save').click();
      });
      await t.settle(200);
      const wakeSet = (await t.calls('settings', 'voice:set')).map((c) => c.payload);
      t.check(
        'voiceWakeSaves',
        wakeSet.some((p) => p.wake === true) && wakeSet.some((p) => p.wakeWord === 'Jarvis'),
        JSON.stringify(wakeSet),
      );
    },
  },
];
