// The agents lane's ui-shots scenes, mocks, checks and CSS lint list: the
// notification that drops from the top of the screen when a coding agent
// finishes or needs you, placed and behaving like Aevalsistant's on
// Aevaleron's glass, and Settings' Agents page. The shape is documented at the top of tools/ui-shots/shots.mjs.

const PREFS = {
  enabled: true,
  agentsOff: ['cursor'],
  chats: false,
  keepAwake: true,
  displayOn: false,
  lidAwake: false,
  lidSaved: '',
  notifyDone: true,
  notifyBackgroundDone: false,
  notifyNeedsYou: true,
  notifyBackgroundNeedsYou: true,
  sound: true,
  expandOnHover: true,
  altTab: true,
  cardTime: 1,
  showChildren: true,
  offered: true,
};

const SNAPSHOT = {
  prefs: PREFS,
  agents: [
    { id: 'claude', name: 'Claude Code', found: true, on: true, connected: true, note: 'Restart open sessions once.', error: null },
    { id: 'codex', name: 'Codex', found: true, on: true, connected: false, note: 'Run /hooks once in Codex to trust the new hooks.', error: 'hooks.json is not a JSON object' },
    { id: 'gemini', name: 'Gemini CLI', found: false, on: true, connected: false, note: '', error: null },
    { id: 'cursor', name: 'Cursor', found: true, on: false, connected: false, note: '', error: null },
  ],
  aevalsistant: false,
  chatProblem: null,
  hasLid: true,
  working: 2,
  awake: true,
  screenOn: false,
};

export const mocks = {
  'agents:get': SNAPSHOT,
  'agents:set': SNAPSHOT,
  'agents:ready': { ok: true },
  'agents:size': { ok: true },
  'agents:pass': { ok: true },
  'agents:hover': { ok: true },
  'agents:open': { ok: true },
  'agents:dismiss': { ok: true },
  'agents:test': { ok: true },
};

export const cssLintEnforce = ['renderer/agents.css'];

export const checks = [
  // The window never takes the keyboard, so nothing in it may depend on
  // focus.
  {
    name: 'watchCardNeedsNoFocus',
    run: (ctx) => !/\.focus\(/.test(ctx.read('renderer/agents.js')) || 'agents.js focuses something',
  },
  // Aevaleron's glass, by the owner's word, with no avatar or drawn face,
  // and only what Aevalsistant's notification has: no buttons, no close box.
  {
    name: 'watchNotificationIsGlass',
    run: (ctx) => {
      const html = ctx.read('renderer/agents.html');
      if (/<button|id="close"|id="actions"/.test(html)) return 'agents.html has buttons';
      if (!/href="glass\.css"/.test(html) || !/src="glass\.js"/.test(html)) return 'agents.html does not load glass.css and glass.js';
      if (!/id="card" class="glass thin"/.test(html)) return 'the card is not thin glass';
      if (/avatar|<svg|<img/.test(html)) return 'agents.html has an icon';
      if (/avatar|createElementNS/.test(ctx.read('renderer/agents.js'))) return 'agents.js draws an icon';
      return true;
    },
  },
];

// The notification's window as Rust places it on the 1600 px desktop: 464
// wide (the 420 card and 22 each side for its shadow), centred, against the
// top of the work area. The card sits 16 down inside it.
const WIN = { x: 568, y: 0, w: 464, h: 300 };
const CLIP = { x: WIN.x - 20, y: 0, width: WIN.w + 40, height: 280 };
// Tall enough for a handful of sessions and their subagents.
const TALL = { ...WIN, h: 520 };
const TALL_CLIP = { ...CLIP, height: 500 };
const S = { x: 120, y: 130, w: 960, h: 700 };

const ROWS = [
  { session: 'a', depth: 0, name: 'site', place: 'Codex · Terminal', state: 'needs-you', text: 'needs you' },
  { session: 'b', depth: 0, name: 'aevalrena', place: 'VS Code', state: 'done', text: 'done 3m ago' },
  { session: 'c', depth: 0, name: 'rover', place: 'Terminal', state: 'working', text: 'working 12m, 1 subagent' },
  { session: 'c', subagent: 'x1', depth: 1, name: 'Explore', place: '', state: 'working', text: 'running 4m' },
];

const TREE = [
  { session: 's1', subagent: 'x1', depth: 1, name: 'Explore', place: '', state: 'done', text: 'done 1m ago' },
  { session: 's1', subagent: 'x2', depth: 1, name: 'Plan', place: '', state: 'working', text: 'running 6m' },
  { session: 'a', depth: 0, name: 'site', place: 'Codex · Terminal', state: 'needs-you', text: 'needs you' },
  { session: 'b', depth: 0, name: 'aevalrena', place: 'VS Code', state: 'done', text: 'done 3m ago' },
  { session: 'b', subagent: 'y1', depth: 1, name: 'general-purpose', place: '', state: 'done', text: 'done 4m ago' },
  { session: 'c', depth: 0, name: 'rover', place: 'Terminal', state: 'working', text: 'working 12m, 2 subagents' },
  { session: 'c', subagent: 'z1', depth: 1, name: 'Explore', place: '', state: 'working', text: 'running 4m' },
  { session: 'c', subagent: 'z2', depth: 1, name: 'code-review', place: '', state: 'working', text: 'running 2m' },
  { session: 'd', depth: 0, name: 'notes', place: 'Windows Terminal', state: 'working', text: 'working 41m' },
  { session: 'e', depth: 0, name: 'tour', place: 'Cursor', state: 'done', text: 'done 18m ago' },
];

const DONE = {
  kind: 'done',
  title: 'aevaleron finished',
  place: 'VS Code',
  detail: 'The tour card is redesigned and the checks pass. Two things are left for you to decide before the release, both in the lane note.',
  session: 's1',
  altTab: true,
  expand: true,
  rows: ROWS,
  more: 2,
};

const facts = (f) =>
  f.evaluate(() => {
    const card = document.getElementById('card');
    const style = getComputedStyle(card);
    const box = card.getBoundingClientRect();
    return {
      shown: !card.hidden && card.classList.contains('in'),
      kind: card.dataset.kind,
      title: document.getElementById('title').textContent,
      place: document.getElementById('place').textContent,
      keys: !document.getElementById('keys').hidden,
      dot: getComputedStyle(document.getElementById('state')).display !== 'none',
      rows: [...document.querySelectorAll('#list .row[data-i]')].map((r) => r.className),
      times: [...document.querySelectorAll('#list .row[data-i] .status')].map((n) => n.textContent),
      glass: card.classList.contains('glass') && !!card.querySelector(':scope > .gl'),
      icon: document.querySelectorAll('#avatar, svg, img, canvas').length,
      tone: document.documentElement.dataset.tone || '',
      more: document.querySelector('#list .row.more')?.textContent || '',
      buttons: document.querySelectorAll('button').length,
      at: [Math.round(box.left), Math.round(box.top), Math.round(box.width)],
      radius: style.borderTopLeftRadius,
      font: getComputedStyle(document.getElementById('title')).fontFamily,
      oneLine: document.getElementById('detail').getBoundingClientRect().height <= 20,
      wide: document.documentElement.scrollWidth - document.documentElement.clientWidth,
      closed: card.offsetHeight,
    };
  });

function cardScene(name, payload, expect, opts = {}) {
  const win = opts.win || WIN;
  const clip = opts.clip || CLIP;
  return {
    name,
    backdrop: opts.backdrop || 'wallpaper',
    windows: [{ name: 'card', src: '/renderer/agents.html', ...win, z: 1, fixed: true }],
    async run(t) {
      const f = t.frame('card');
      t.check(`${name}.ready`, (await t.calls('card', 'agents:ready')).length === 1, 'agents.js never invoked agents:ready');
      await t.emit('card', 'agents:card', payload);
      await t.settle(900);
      // Rust repeats the desktop's frame; two running turn the glass light
      // over a light desktop.
      if (opts.tone) {
        for (let i = 0; i < 2; i += 1) {
          await f.evaluate(({ img }) => window.__emit('glass:frame', { img, display: { x: 0, y: 0, width: 1600, height: 1000 }, tier: 'live', reasons: [] }), { img: t.backdrop.small });
          await t.settle(200);
        }
        await t.settle(450);
      }
      const c = await facts(f);
      const sized = (await t.calls('card', 'agents:size')).map((x) => x.payload.height);
      // The window is as tall as the card open: 16 above, 22 below.
      t.check(`${name}.sized`, sized.length >= 1 && sized[0] >= c.closed + 38, `agents:size heights ${JSON.stringify(sized)} for a card ${c.closed} tall`);
      t.check(
        `${name}.drawn`,
        c.shown && c.kind === payload.kind && c.title === payload.title && c.glass && c.icon === 0 && c.radius === '28px' && c.at.join() === '22,16,420' && c.wide === 0,
        JSON.stringify(c),
      );
      t.check(`${name}.rows`, c.rows.length === expect.rows && c.more === expect.more, JSON.stringify([c.rows, c.more]));
      // Each row carries its own state and time, as Rust sent it.
      const sent = (payload.rows || []).map((r) => r.text);
      t.check(`${name}.times`, c.times.join('|') === sent.join('|'), JSON.stringify([c.times, sent]));
      if (opts.tone) t.check(`${name}.tone`, c.tone === opts.tone, `tone ${c.tone}`);
      t.check(`${name}.parts`, c.keys === expect.keys && c.dot === expect.dot && c.buttons === 0, JSON.stringify([c.keys, c.dot, c.buttons]));
      await t.shot(name, clip);
      return c;
    },
  };
}

const done = cardScene('watch-card-done', DONE, { rows: 4, more: '2 more in the tray menu', keys: true, dot: true });
const offer = cardScene(
  'watch-card-offer',
  {
    kind: 'info',
    title: 'Watch your coding agents?',
    place: '',
    detail: 'Aevaleron can keep this PC awake while Claude Code, Codex and the others work, and tell you when one finishes or needs you. Click to turn it on in Settings.',
    session: null,
    altTab: false,
    expand: true,
    rows: [],
    more: 0,
  },
  { rows: 0, more: '', keys: false, dot: false },
);

export const scenes = [
  {
    ...done,
    async run(t) {
      const c = await done.run(t);
      t.check('watchDetailOneLineAtRest', c.oneLine, 'the detail shows more than one line before the pointer rests on it');
      const f = t.frame('card');
      // Off the card the window lets clicks through; on it, it takes them.
      await t.page.mouse.move(WIN.x + 8, 250);
      await t.settle(100);
      await t.page.mouse.move(WIN.x + 200, 40);
      await t.settle(500);
      const pass = (await t.calls('card', 'agents:pass')).map((x) => x.payload.ignore);
      t.check('watchClicksGoThroughOffTheCard', pass[0] === true && pass[pass.length - 1] === false, JSON.stringify(pass));
      // The pointer resting on the card holds the countdown and opens the
      // whole message.
      const open = await f.evaluate(() => ({
        open: document.getElementById('card').classList.contains('open'),
        hover: document.getElementById('card').classList.contains('hover'),
        tall: document.getElementById('detail').getBoundingClientRect().height > 20,
      }));
      const hover = (await t.calls('card', 'agents:hover')).map((x) => x.payload.on);
      t.check('watchHoverOpensTheMessage', open.open && open.hover && open.tall && hover[0] === true, JSON.stringify([open, hover]));
      await t.shot('watch-card-open', CLIP);
      // A row goes to its session's window, a subagent's row to the window
      // its session runs in.
      await f.click('#list .row.needs-you');
      await f.click('#list .row.child');
      const opened = (await t.calls('card', 'agents:open')).map((x) => x.payload.session);
      t.check('watchRowOpensItsSession', opened.join() === 'a,c', JSON.stringify(opened));
      // A right click puts it away.
      await f.click('#head', { button: 'right' });
      t.check('watchRightClickDismisses', (await t.calls('card', 'agents:dismiss')).length === 1, 'no agents:dismiss');
      await t.page.mouse.move(0, 600);
      await t.emit('card', 'agents:hide', null);
      await t.settle(100);
      const early = (await t.calls('card', 'agents:hidden')).length;
      await t.settle(400);
      const out = await f.evaluate(() => document.getElementById('card').classList.contains('out'));
      t.check('watchCardLiftsAway', out, 'agents:hide did not lift the card away');
      // The window hides when the page says the card is gone, not on a
      // timer: said once, after the lift, with the card out of the page.
      const gone = await f.evaluate(() => document.getElementById('card').hidden);
      const said = (await t.calls('card', 'agents:hidden')).length;
      t.check('watchCardSaysHidden', early === 0 && gone && said === 1, `early=${early} hidden=${gone} said=${said}`);
    },
  },
  cardScene(
    'watch-card-needs-you',
    { ...DONE, kind: 'needs-you', title: 'site needs you', place: 'Codex · Terminal', detail: 'Codex wants to use shell', session: 'a', rows: ROWS.slice(1, 4), more: 0, altTab: false },
    { rows: 3, more: '', keys: false, dot: true },
  ),
  // A handful of sessions, each with what it started nested under it, each
  // row with its own state and time, on a white desktop for the light tone.
  cardScene(
    'watch-card-tree',
    { ...DONE, title: 'aevaleron finished', rows: TREE, more: 3, altTab: false },
    { rows: TREE.length, more: '3 more in the tray menu', keys: false, dot: true },
    { win: TALL, clip: TALL_CLIP, backdrop: 'white', tone: 'light' },
  ),
  {
    ...offer,
    async run(t) {
      await offer.run(t);
      // The app's own message has no buttons: a click on it opens Settings.
      await t.frame('card').click('#head');
      const opened = (await t.calls('card', 'agents:open')).map((x) => x.payload);
      t.check('watchOfferOpensSettings', opened.length === 1 && opened[0].session === undefined, JSON.stringify(opened));
    },
  },

  // Settings, Agents: each agent found on this PC, keep-awake and the
  // notifications.
  {
    name: 'watch-settings',
    backdrop: 'wallpaper',
    textScale: true,
    windows: [{ name: 'settings', src: '/settings/settings.html', ...S, z: 1 }],
    async run(t) {
      const st = t.frame('settings');
      await t.settle(300);
      await st.evaluate(() => {
        showPage('pg-watch');
        openSection('sec-watch-agents');
      });
      await t.settle(600);
      await t.shot('watch-settings-agents', { x: S.x - 10, y: S.y - 10, width: S.w + 20, height: S.h + 20 });
      const page = await st.evaluate(() => ({
        enabled: document.getElementById('watch-enabled').checked,
        summary: document.getElementById('watch-agents-summary').textContent,
        agents: [...document.querySelectorAll('#watch-agent-list .switch')].map((l) => l.firstChild.firstChild.textContent),
        says: [...document.querySelectorAll('#watch-agent-list .watch-says')].map((n) => n.textContent),
        status: document.getElementById('watch-status').textContent,
        awake: document.getElementById('watch-awake-summary').textContent,
        cards: document.getElementById('watch-cards-summary').textContent,
        lid: !document.getElementById('watch-lid-row').hidden,
        time: document.querySelector('[data-watch-time][aria-checked="true"]')?.dataset.watchTime,
      }));
      t.check(
        'watchSettingsPage',
        page.enabled && page.summary === '1 of 3 connected' && page.agents.join() === 'Claude Code,Codex,Cursor' && page.says.length === 2 && /Could not connect Codex/.test(page.status) && page.awake === 'On · holding now' && page.cards === 'Finished and needs you' && page.lid && page.time === '1',
        JSON.stringify(page),
      );
      // Chat watching: off until asked for, saved by its switch, and when it
      // stopped by itself the reason shows under the switch.
      const chat = () =>
        st.evaluate(() => {
          const box = document.querySelector('input[data-watch="chats"]');
          const says = document.getElementById('watch-chats-status');
          return { label: box.closest('label').textContent.trim(), shown: !box.closest('label').hidden, on: box.checked, says: says.hidden ? '' : says.textContent };
        });
      const chatOff = await chat();
      await st.click('input[data-watch="chats"]');
      await t.settle(200);
      const chatSet = (await t.calls('settings', 'agents:set')).map((c) => c.payload);
      t.check(
        'watchChatsSwitch',
        chatOff.label === 'Also watch the Claude and ChatGPT apps' && chatOff.shown && !chatOff.on && chatOff.says === '' && chatSet.some((p) => p.chats === true),
        JSON.stringify([chatOff, chatSet]),
      );
      const stopped = 'Chat watching stopped: Windows UI Automation did not load.';
      await t.emit('settings', 'agents:state', { ...SNAPSHOT, prefs: { ...PREFS, chats: true }, chatProblem: stopped });
      await t.settle(200);
      const chatOn = await chat();
      t.check('watchChatsSaysWhyItStopped', chatOn.on && chatOn.says === stopped, JSON.stringify(chatOn));
      await t.shot('watch-settings-chats', { x: S.x - 10, y: S.y - 10, width: S.w + 20, height: S.h + 20 });
      // With agent watching off there is nothing for it to add to.
      await t.emit('settings', 'agents:state', { ...SNAPSHOT, prefs: { ...PREFS, enabled: false, chats: true }, chatProblem: stopped });
      await t.settle(200);
      const chatHidden = await chat();
      t.check('watchChatsNeedsWatching', !chatHidden.shown && chatHidden.says === '', JSON.stringify(chatHidden));
      await t.emit('settings', 'agents:state', SNAPSHOT);
      await t.settle(200);
      await st.evaluate(() => openSection('sec-watch-awake'));
      await t.settle(400);
      await st.click('input[data-watch="displayOn"]');
      await t.settle(200);
      const set = (await t.calls('settings', 'agents:set')).map((c) => c.payload);
      t.check('watchSettingsSaves', set.some((p) => p.displayOn === true), JSON.stringify(set));
      await t.shot('watch-settings-awake', { x: S.x - 10, y: S.y - 10, width: S.w + 20, height: S.h + 20 });
      // With the lid kept working, the note says what happens once the
      // agents stop and the lid is still shut.
      await t.emit('settings', 'agents:state', { ...SNAPSHOT, prefs: { ...PREFS, lidAwake: true } });
      await t.settle(200);
      const lid = await st.evaluate(() => ({
        note: document.getElementById('watch-awake-note').textContent,
        summary: document.getElementById('watch-awake-summary').textContent,
        on: document.querySelector('input[data-watch="lidAwake"]').checked,
      }));
      t.check('watchLidNoteSaysItSleeps', lid.on && /a minute after the last one stops, the PC sleeps or hibernates/.test(lid.note) && lid.summary === 'On, lid closed too · holding now', JSON.stringify(lid));
      await t.emit('settings', 'agents:state', SNAPSHOT);
      await t.settle(200);
    },
  },
];
