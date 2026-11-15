// The region-text lane's ui-shots scenes, mocks, checks and CSS lint list:
// the area picker when it is opened to copy text, the note that says what
// was copied, and the shortcut row in Settings. The shape is documented at
// the top of tools/ui-shots/shots.mjs; shared fixtures are in
// tools/ui-shots/kit.mjs.
import { W, H, COMMON } from '../kit.mjs';

const BASE = COMMON['settings:get'];

export const mocks = {
  'region:note-ready': null,
  // The action has no shortcut until the person sets one.
  'settings:get': {
    shortcuts: { copyRegionText: '' },
    active: { copyRegionText: null },
    defaults: { copyRegionText: '' },
  },
};

export const cssLintEnforce = ['renderer/region-note.css'];

export const checks = [
  // The note only listens, and says once that it is ready to.
  {
    name: 'regionNoteOnlyReports',
    run: (ctx) => {
      const called = [...ctx.read('renderer/region-note.js').matchAll(/aeval\.invoke\('([^']+)'/g)].map((m) => m[1]);
      return called.join() === 'region:note-ready' || `region-note.js invokes ${called.join(', ')}`;
    },
  },
  // Nothing about the copy is kept by the page.
  {
    name: 'regionNoteKeepsNothing',
    run: (ctx) => !/localStorage|sessionStorage|indexedDB|fetch\(/.test(ctx.read('renderer/region-note.js')) || 'region-note.js stores or sends something',
  },
];

// The picker covers a whole display.
const picker = { name: 'region', src: '/renderer/region.html', x: 0, y: 0, w: W, h: H, z: 1, fixed: true };
// The note's window is 380 x 92 (region_text/note.rs), here under a
// rectangle someone drew in the middle of a busy screen.
const NOTE = { x: 610, y: 520, w: 380, h: 92 };
const note = { name: 'note', src: '/renderer/region-note.html', ...NOTE, z: 1 };
const NOTE_CLIP = { x: NOTE.x - 40, y: NOTE.y - 24, width: NOTE.w + 80, height: NOTE.h + 48 };
const S = { x: 440, y: 120, w: 700, h: 640 };

// What Rust sends for each kind (region_text/note.rs), held on screen (ms 0)
// for the picture.
const COPIED = { kind: 'copied', title: 'Copied 42 words', lead: 'Invoice 2291 for March Please settle…', ms: 0 };
const NOTHING = { kind: 'none', title: 'No text found here', lead: 'The clipboard is unchanged.', ms: 0 };
const READING = { kind: 'reading', title: 'Reading the text…', lead: '', ms: 0 };
const NO_OCR = {
  kind: 'error',
  title: 'Windows cannot read text in pictures yet',
  lead: 'Add a language with text recognition in Windows Settings, then try again.',
  ms: 0,
};

const key = (f, k, alt) => f.evaluate(([k, alt]) => document.dispatchEvent(new KeyboardEvent('keydown', { key: k, altKey: !!alt, bubbles: true })), [k, alt]);

// What the note shows, and whether all of it is inside its window.
const noteFacts = (f) =>
  f.evaluate(() => {
    const box = document.getElementById('note').getBoundingClientRect();
    const lead = document.getElementById('lead');
    return {
      kind: document.body.dataset.kind,
      title: document.getElementById('title').textContent,
      lead: lead.textContent,
      leadShown: getComputedStyle(lead).display !== 'none',
      tick: getComputedStyle(document.getElementById('mark')).display !== 'none',
      opacity: getComputedStyle(document.getElementById('note')).opacity,
      inside: box.left >= 0 && box.top >= 0 && box.right <= window.innerWidth && box.bottom <= window.innerHeight,
      role: document.getElementById('words').getAttribute('role'),
      focusable: document.querySelectorAll('button, a, input, [tabindex]').length,
    };
  });

// One scene per kind of note, so each is audited as it is left.
function noteScene(name, payload, expect) {
  return {
    name,
    backdrop: 'photo-busy',
    windows: [note],
    async run(t) {
      const f = t.frame('note');
      await t.settle(200);
      t.check(`${name}.ready`, (await t.calls('note', 'region:note-ready')).length === 1, 'region-note.js never invoked region:note-ready');
      const before = await noteFacts(f);
      t.check(`${name}.hiddenFirst`, before.kind === 'idle' && before.opacity === '0', JSON.stringify(before));
      await t.emit('note', 'region:note', payload);
      await t.settle(400);
      const n = await noteFacts(f);
      const ok = n.kind === payload.kind && n.title === payload.title && n.lead === payload.lead && n.opacity === '1' && n.inside;
      t.check(`${name}.says`, ok && n.tick === expect.tick && n.leadShown === !!payload.lead, JSON.stringify(n));
      // It is read out, and there is nothing in it to focus or press.
      t.check(`${name}.statusOnly`, n.role === 'status' && n.focusable === 0, JSON.stringify(n));
      await t.shot(name, NOTE_CLIP);
    },
  };
}

export const scenes = [
  // The picker opened by "Copy text from screen": its one line says what the
  // drag will do. Rust sets window.aevalRegionFor before the page's scripts
  // run (region.rs page_script); here an init script does, and the picker
  // loads again with it.
  {
    name: 'region-text-picker',
    backdrop: 'white',
    windows: [picker],
    async run(t) {
      await t.page.addInitScript(() => {
        if (window.frameElement && window.frameElement.name === 'region') window.aevalRegionFor = 'text';
      });
      await t.page.evaluate(() => {
        const frame = document.querySelector('iframe[name="region"]');
        frame.src = frame.getAttribute('src');
      });
      await t.frame('region').waitForFunction(() => window.aevalRegionFor === 'text' && document.readyState === 'complete' && !!document.getElementById('hint-text'));
      await t.settle(400);
      const f = t.frame('region');
      const hint = await f.evaluate(() => document.getElementById('hint').textContent.replace(/\s+/g, ' ').trim());
      t.check('regionTextHint', hint === 'Drag over the text to copy it Esc Arrows move, Alt+arrows resize', hint);
      const label = await f.evaluate(() => `${document.body.getAttribute('role')}|${document.body.getAttribute('aria-label')}`);
      t.check('regionTextRole', label === 'application|Choose an area of the screen to copy its text', label);
      await t.shot('region-text-picker', { x: 400, y: 0, width: 800, height: 120 });
      // Without a mouse: the first key says what Enter will do here.
      await key(f, 'ArrowRight');
      const said = await f.evaluate(() => document.getElementById('live').textContent);
      const want = `Area ${W / 2} by ${H / 2}. Arrows move it, Alt+arrows resize, Enter copies the text, Esc cancels.`;
      t.check('regionTextAnnounces', said === want, said);
      // Enter answers with the box, as a drag does.
      await key(f, 'Enter');
      const done = (await t.calls('region', 'region:done')).map((c) => c.payload);
      t.check('regionTextEnterAnswers', done.length === 1 && done[0].width === W / 2 && done[0].height === H / 2, JSON.stringify(done));
    },
  },

  noteScene('region-text-note-copied', COPIED, { tick: true }),
  noteScene('region-text-note-none', NOTHING, { tick: false }),
  noteScene('region-text-note-reading', READING, { tick: false }),
  noteScene('region-text-note-error', NO_OCR, { tick: false }),

  // The longest things the note can say stay inside it, and a note with a
  // time fades when that time is up.
  {
    name: 'region-text-note-long',
    backdrop: 'photo-busy',
    windows: [note],
    async run(t) {
      const f = t.frame('note');
      await t.settle(200);
      // Forty wide letters, the most Rust sends as the first words.
      const long = { kind: 'copied', title: 'Copied 1204 characters', lead: `${'WMWMWMWMWM '.repeat(3)}WMWMWMW…`, ms: 0 };
      await t.emit('note', 'region:note', long);
      await t.settle(400);
      const n = await noteFacts(f);
      const cut = await f.evaluate(() => {
        const lead = document.getElementById('lead');
        return lead.scrollWidth > lead.clientWidth;
      });
      t.check('regionNoteLongStaysInside', n.inside && n.opacity === '1' && cut, JSON.stringify({ n, cut }));
      await t.shot('region-text-note-long', NOTE_CLIP);
      await t.emit('note', 'region:note', { ...COPIED, ms: 300 });
      await t.settle(150);
      const up = await noteFacts(f);
      await t.settle(700);
      const gone = await noteFacts(f);
      t.check('regionNoteLeaves', up.kind === 'copied' && gone.kind === 'idle' && gone.opacity === '0', JSON.stringify({ up: up.kind, gone }));
      // A newer note takes the place of one still fading in, and its own time
      // counts, not the older one's.
      await t.emit('note', 'region:note', { ...READING });
      await t.emit('note', 'region:note', { ...NOTHING });
      await t.settle(500);
      const last = await noteFacts(f);
      t.check('regionNoteNewestWins', last.kind === 'none' && last.title === NOTHING.title, JSON.stringify(last));
    },
  },

  // Settings, Shortcuts: the action's row, unset until the person sets it.
  {
    name: 'region-text-shortcut',
    backdrop: 'wallpaper',
    windows: [{ name: 'settings', src: '/settings/settings.html', ...S, z: 1, responses: {} }],
    async run(t) {
      const f = t.frame('settings');
      await t.settle(300);
      await f.evaluate(() => {
        const main = document.querySelector('main');
        const el = document.getElementById('pg-shortcuts');
        main.style.scrollBehavior = 'auto';
        main.scrollTop = el.offsetTop - main.offsetTop;
        main.dispatchEvent(new Event('scroll'));
      });
      await t.settle(150);
      await f.click('#sec-shortcuts > .row-head > .edit');
      await t.settle(450);
      const row = () =>
        f.evaluate(() => {
          const r = document.querySelector('.sc-row[data-action="copyRegionText"]');
          const rows = [...document.querySelectorAll('.sc-row[data-action]')].map((x) => x.dataset.action);
          return {
            label: r.querySelector('.sc-label').textContent,
            chord: r.querySelector('.sc-current').textContent,
            empty: r.querySelector('.sc-current').classList.contains('empty'),
            button: r.querySelector('.sc-change').textContent,
            after: rows[rows.indexOf('copyRegionText') - 1],
            warn: !r.querySelector('.sc-warn').hidden,
          };
        });
      const unset = await row();
      t.check(
        'regionTextShortcutRow',
        unset.label === 'Copy text from screen' && unset.chord === 'Not set' && unset.empty && unset.after === 'captureRegion' && !unset.warn,
        JSON.stringify(unset),
      );
      await t.shot('region-text-shortcut-unset');
      await f.click('.sc-row[data-action="copyRegionText"] .sc-change');
      await t.settle(150);
      const asking = await row();
      t.check('regionTextShortcutCapturing', asking.chord === 'Press keys' && asking.button === 'Cancel', JSON.stringify(asking));
      await t.respond('settings', 'shortcuts:set', {
        ok: true,
        shortcuts: { ...BASE.shortcuts, copyRegionText: 'Ctrl+Alt+T' },
        active: { ...BASE.active, copyRegionText: true },
      });
      await t.page.keyboard.press('Control+Alt+T');
      await t.settle(250);
      const sets = (await t.calls('settings', 'shortcuts:set')).map((c) => c.payload);
      const set = await row();
      t.check(
        'regionTextShortcutSaves',
        sets.length === 1 && sets[0].action === 'copyRegionText' && sets[0].accelerator === 'Ctrl+Alt+T' && set.chord === 'Ctrl+Alt+T' && !set.empty,
        JSON.stringify({ sets, set }),
      );
      await t.shot('region-text-shortcut-set');
    },
  },
];
