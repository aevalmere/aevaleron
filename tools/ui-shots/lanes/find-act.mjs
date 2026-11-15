// The find-act lane's ui-shots scenes: what Enter will do on each Find
// result, a menu command among the results, and what the bar says when the
// match Enter was pressed on has gone. The shape is documented at the top of
// tools/ui-shots/shots.mjs.
import { BAR, OUT1, W, H, openBar } from '../kit.mjs';

export const mocks = {};
export const checks = [];
export const cssLintEnforce = [];

const overlay = { name: 'overlay', src: '/renderer/index.html', x: BAR.x, y: BAR.y, w: BAR.w, h: BAR.h, z: 2 };
const out1 = { name: 'out1', src: '/renderer/output.html', x: OUT1.x, y: OUT1.y, w: OUT1.w, h: OUT1.h, z: 2 };
const BAR_CLIP = { x: BAR.x - 20, y: 0, width: BAR.w + 40, height: 200 };

// What Rust says when the match is no longer there (find/act.rs GONE).
const GONE = 'That is no longer in the window, so nothing was done. The matches were read again.';

// Events in order, in one window.
const emitAll = (f, list) =>
  f.evaluate((l) => {
    for (const [c, p] of l) window.__emit(c, p);
  }, list);

// A window's frame plus a margin, in scene pixels, as the mock last sized it.
const clipOf = (t, name, pad = 20) =>
  t.page.evaluate(
    ({ n, p, w, h }) => {
      const r = document.querySelector(`iframe[name="${n}"]`).getBoundingClientRect();
      const x = Math.max(0, Math.floor(r.left - p));
      const y = Math.max(0, Math.floor(r.top - p));
      return { x, y, width: Math.min(w, Math.ceil(r.right + p)) - x, height: Math.min(h, Math.ceil(r.bottom + p)) - y };
    },
    { n: name, p: pad, w: W, h: H }
  );

const payloads = async (t, name, channel) => (await t.calls(name, channel)).map((c) => c.payload || {});

// Each row as the page drew it: its words, and where the action word sits.
const rowsOf = (f) =>
  f.evaluate(() =>
    [...document.querySelectorAll('.find-row')].map((row) => {
      const act = row.querySelector('.find-act');
      const snip = row.querySelector('.find-snippet');
      const mark = row.querySelector('.kind-mark');
      const why = row.querySelector('.find-why');
      const r = row.getBoundingClientRect();
      const a = act ? act.getBoundingClientRect() : null;
      const s = snip.getBoundingClientRect();
      return {
        act: act ? act.textContent : '',
        why: why ? why.textContent : '',
        menu: mark.classList.contains('menu'),
        // The word is inside the row, after the snippet, on its first line.
        placed: !a || (a.right <= r.right + 0.5 && a.left >= s.right - 0.5 && Math.abs(a.top - s.top) <= 6 && a.width > 0),
        edges: act ? getComputedStyle(act).borderTopWidth + getComputedStyle(act).backgroundImage : '',
      };
    })
  );

const ROWS = {
  gen: 4,
  query: 'save',
  current: 0,
  hits: [
    { i: 0, kind: 'literal', text: 'Save', match: 'Save', act: 'Press' },
    { i: 1, kind: 'literal', text: 'Save as type', match: 'Save', act: 'Focus' },
    { i: 2, kind: 'literal', text: 'Remember to save the draft before Friday', match: 'save', act: 'Select' },
    { i: 3, kind: 'literal', text: 'Autosave is on', match: 'save', act: 'Click' },
    { i: 4, kind: 'related', text: 'Keep a copy', why: 'Another way to say save', act: 'Press' },
    { i: 5, kind: 'literal', text: 'Saved two minutes ago', match: 'Save', offscreen: true, act: '' },
  ],
};

const MENU_ROWS = {
  gen: 5,
  query: 'wrap',
  current: 0,
  hits: [
    { i: 0, kind: 'literal', text: 'Text wrapping breaks long lines at the window edge', match: 'wrap', act: 'Select' },
    // Rust marks a menu command as not on screen, as it does a match that
    // is scrolled away.
    { i: 1, kind: 'menu', text: 'Format › Word Wrap', match: 'Wrap', why: 'Menu command', act: 'Press', offscreen: true },
    { i: 2, kind: 'menu', text: 'View › Wrap Around Search', match: 'Wrap', why: 'Menu command', act: 'Press', offscreen: true },
  ],
};

export const scenes = [
  // Each row ends in the one word for what Enter will do; a row Enter can
  // do nothing with has none. Enter on a row acts on that row.
  {
    name: 'find-act-rows',
    backdrop: 'wallpaper',
    textScale: true,
    windows: [out1],
    async run(t) {
      const f = t.frame('out1');
      await emitAll(f, [
        ['overlay:state', { mode: 'panel', chatMax: 520 }],
        ['view:set', { view: 'find' }],
        ['find:state', { gen: 4, open: true, scope: 'window', status: '5 exact · 1 related', lines: [], counts: { exact: 5, related: 1, menu: 0 } }],
        ['find:hits', ROWS],
      ]);
      await t.settle(500);
      await t.shot('find-act-rows', await clipOf(t, 'out1'));
      const rows = await rowsOf(f);
      const words = rows.map((r) => r.act).join(',');
      t.check('findRowsSayTheAction', words === 'Press,Focus,Select,Click,Press,', words);
      const quiet = rows.every((r) => r.placed && (!r.act || r.edges === '0pxnone'));
      t.check('findActionWordIsQuiet', quiet, JSON.stringify(rows));

      // Enter on the current row, then on the next one down.
      await f.focus('.find-row.current');
      await t.page.keyboard.press('Enter');
      await t.settle(100);
      await t.page.keyboard.press('ArrowDown');
      await t.settle(100);
      await t.page.keyboard.press('Enter');
      await t.settle(200);
      const acts = await payloads(t, 'out1', 'find:act');
      const steps = (await payloads(t, 'out1', 'find:step')).map((p) => p.by && p.by.index);
      const want = [
        { gen: 4, index: 0, match: 'Save' },
        { gen: 4, index: 1, match: 'Save' },
      ];
      t.check('findRowEnterActs', JSON.stringify(acts) === JSON.stringify(want) && steps.includes(1), JSON.stringify({ acts, steps }));
      // A click still only goes to the match.
      await f.click('.find-row[data-i="2"]');
      await t.settle(100);
      const after = (await payloads(t, 'out1', 'find:act')).length;
      t.check('findRowClickOnlyGoesThere', after === 2, `${after} find:act calls`);
    },
  },

  // Menu commands come after what is on screen, each marked as one and
  // with its path from the menu bar down.
  {
    name: 'find-act-menu',
    backdrop: 'wallpaper',
    windows: [out1],
    async run(t) {
      const f = t.frame('out1');
      await emitAll(f, [
        ['overlay:state', { mode: 'panel', chatMax: 520 }],
        ['view:set', { view: 'find' }],
        ['find:state', { gen: 5, open: true, scope: 'window', status: '1 exact · 2 menu commands', lines: [], counts: { exact: 1, related: 0, menu: 2 } }],
        ['find:hits', MENU_ROWS],
      ]);
      await t.settle(500);
      await t.shot('find-act-menu', await clipOf(t, 'out1'));
      const rows = await rowsOf(f);
      const marks = rows.map((r) => (r.menu ? 'menu' : 'screen')).join(',');
      const said = rows.filter((r) => r.menu).every((r) => r.why === 'Menu command' && r.act === 'Press' && r.placed);
      t.check('findMenuRowsComeLast', marks === 'screen,menu,menu' && said, JSON.stringify(rows));
    },
  },

  // Enter in the bar acts on the selected match, the arrows and F3 step,
  // and a match that has gone is said in the bar's status row.
  {
    name: 'find-act-gone',
    backdrop: 'wallpaper',
    windows: [overlay],
    async run(t) {
      const f = t.frame('overlay');
      const status = () => f.evaluate(() => document.getElementById('bar-status-text').textContent);
      await f.evaluate(() => localStorage.setItem('aeval.hasSentFirstRequest', '1'));
      await openBar(f);
      await t.settle();
      await f.evaluate(() => document.getElementById('query-input').focus());
      await t.page.keyboard.type('/total');
      await t.settle(400);
      const hits = {
        gen: 7,
        query: 'total',
        current: 0,
        hits: [
          { i: 0, kind: 'literal', text: 'Invoice total: €1,240.00', match: 'total', act: 'Select' },
          { i: 1, kind: 'literal', text: 'Subtotal', match: 'total', act: 'Select' },
          { i: 2, kind: 'menu', text: 'View › Totals Row', match: 'Total', why: 'Menu command', act: 'Press', offscreen: true },
        ],
      };
      const state = { gen: 7, open: true, scope: 'window', status: '2 exact · 1 menu command', lines: [], counts: { exact: 2, related: 0, menu: 1 }, stage: {}, current: 0, total: 3, more: false };
      await emitAll(f, [
        ['find:hits', hits],
        ['find:state', state],
      ]);
      await t.settle(200);
      const counted = await status();
      t.check('findBarCountsMenus', counted === '1 of 3 · 1 in menus', counted);

      for (const k of ['ArrowDown', 'ArrowUp', 'F3', 'Shift+F3']) await t.page.keyboard.press(k);
      await t.settle(100);
      const steps = (await payloads(t, 'overlay', 'find:step')).map((p) => p.by);
      t.check('findArrowsStep', steps.join(',') === '1,-1,1,-1', JSON.stringify(steps));

      await t.respond('overlay', 'find:act', { status: 'gone', message: GONE });
      await t.page.keyboard.press('Enter');
      await t.settle(300);
      const acts = await payloads(t, 'overlay', 'find:act');
      const stepsAfter = (await t.calls('overlay', 'find:step')).length;
      const sent = JSON.stringify(acts) === JSON.stringify([{ gen: 7, index: 0, match: 'total' }]) && stepsAfter === 4;
      t.check('findEnterActs', sent, JSON.stringify({ acts, stepsAfter }));
      const said = await status();
      const kept = await f.evaluate(() => document.getElementById('query-input').value);
      t.check('findGoneIsSaid', said === GONE && kept === 'total', JSON.stringify({ said, kept }));
      await t.shot('find-act-gone', BAR_CLIP);

      // Outside Find, Enter is what it was: a question is asked.
      await f.evaluate(() => {
        const i = document.getElementById('query-input');
        i.focus();
        i.value = '';
        i.dispatchEvent(new Event('input', { bubbles: true }));
      });
      await t.page.keyboard.press('Backspace');
      await t.page.keyboard.type('why is this red?');
      await t.settle(400);
      await t.page.keyboard.press('Enter');
      await t.settle(200);
      const asked = (await payloads(t, 'overlay', 'query:submit')).map((p) => p.question);
      const actsAfter = (await t.calls('overlay', 'find:act')).length;
      t.check('enterOutsideFindAsks', asked.join('|') === 'why is this red?' && actsAfter === 1, JSON.stringify({ asked, actsAfter }));
    },
  },
];
