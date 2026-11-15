// The private apps lane's ui-shots scenes, mocks, checks and CSS lint list:
// Settings' Private apps row (no apps, the starting list, removing, adding
// by name, adding the app that was in front, a name that is not one), the
// bar's notice while a listed app is in front, and "What was sent" for a
// question that went without the screen. The shape is documented at the
// top of tools/ui-shots/shots.mjs.
import { BAR, H, OUT1, W, openBar } from '../kit.mjs';

// The one line every surface says, written by Rust (privacy.rs).
const LINE = 'Notepad is on your private apps list, so the screen was not read.';
const BAD_NAME = "That is not a program name. Type the name of the app's .exe file, for example notepad.exe.";

const app = (exe, name) => ({ exe, name });
// privacy.rs DEFAULTS, as privacy:get answers before the person changes it.
const APPS = [
  app('1password.exe', '1Password'),
  app('bitwarden.exe', 'Bitwarden'),
  app('dashlane.exe', 'Dashlane'),
  app('keepass.exe', 'KeePass'),
  app('keepassxc.exe', 'KeePassXC'),
  app('protonpass.exe', 'Proton Pass'),
  app('sechealthui.exe', 'Windows Security'),
];
const NOTEPAD = app('notepad.exe', 'Notepad');
const front = (listed) => ({ ...NOTEPAD, listed });
const LIST = { apps: APPS, front: front(false) };
const WITH_NOTEPAD = { apps: [...APPS, NOTEPAD], front: front(true) };

export const mocks = {
  'privacy:get': LIST,
  'privacy:add': LIST,
  'privacy:remove': LIST,
};

export const cssLintEnforce = ['settings/settings.css'];

export const checks = [
  // One sentence everywhere: the pages show the line Rust wrote and never
  // write their own.
  {
    name: 'privateLineIsWrittenOnce',
    run: (ctx) => {
      const pages = ['renderer/overlay.js', 'renderer/output.js', 'renderer/private-apps-settings.js', 'settings/settings.html'];
      const own = pages.filter((f) => /so the screen was not read/.test(ctx.read(f)));
      return own.length === 0 || `${own.join(', ')} writes the line itself`;
    },
  },
];

const S = { x: 120, y: 130, w: 960, h: 700 };
const S_CLIP = { x: S.x - 10, y: S.y - 10, width: S.w + 20, height: S.h + 20 };
const settingsWin = (responses) => ({ name: 'settings', src: '/settings/settings.html', ...S, z: 1, responses: responses || {} });
const overlay = { name: 'overlay', src: '/renderer/index.html', x: BAR.x, y: BAR.y, w: BAR.w, h: BAR.h, z: 2 };
const BAR_CLIP = { x: BAR.x - 20, y: 0, width: BAR.w + 40, height: 300 };

// Opens the row the way the bar's notice does, and brings it into view.
async function openRow(t) {
  const st = t.frame('settings');
  await t.settle(300);
  await st.evaluate(() => goToSection('sec-private'));
  await t.settle(700);
  await st.evaluate(() => document.getElementById('sec-private').scrollIntoView({ block: 'start' }));
  await t.settle(200);
  return st;
}

const facts = (st) =>
  st.evaluate(() => {
    const at = (id) => document.getElementById(id);
    const status = at('private-status');
    const active = document.activeElement;
    return {
      open: at('sec-private').dataset.open === '1',
      summary: at('private-summary').textContent,
      apps: [...document.querySelectorAll('#private-list .private-row')].map((r) => `${r.querySelector('.private-app').firstChild.textContent}|${r.querySelector('.private-exe').textContent}`),
      removes: [...document.querySelectorAll('#private-list button')].map((b) => b.getAttribute('aria-label')),
      listShown: !at('private-list').hidden,
      empty: !at('private-empty').hidden,
      frontOff: at('private-add-front').disabled,
      frontNote: at('private-front-note').textContent,
      field: at('private-name').value,
      status: status.hidden ? '' : status.textContent,
      error: status.classList.contains('error'),
      focus: active ? active.id || active.getAttribute('aria-label') || active.tagName : '',
      wide: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    };
  });

// The answer to a change, and to the reload a click's focus can start.
async function answer(t, channel, value) {
  await t.respond('settings', channel, value);
  const { added, removed, error, ...state } = value;
  await t.respond('settings', 'privacy:get', state);
}

const payloads = async (t, channel) => (await t.calls('settings', channel)).map((c) => c.payload);

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

export const scenes = [
  // Every app taken off: the row says so, and the app that was in front
  // cannot be added when none has been.
  {
    name: 'private-apps-empty',
    backdrop: 'wallpaper',
    windows: [settingsWin({ 'privacy:get': { apps: [], front: null } })],
    async run(t) {
      const st = await openRow(t);
      await t.shot('private-apps-empty', S_CLIP);
      const p = await facts(st);
      const ok = p.open && p.summary === 'None' && p.apps.length === 0 && !p.listShown && p.empty && p.frontOff && /^Open the bar over an app/.test(p.frontNote) && p.wide === 0;
      t.check('privateAppsEmpty', ok, JSON.stringify(p));
    },
  },

  // The list as it starts, and one app taken off it.
  {
    name: 'private-apps-list',
    backdrop: 'wallpaper',
    textScale: true,
    windows: [settingsWin()],
    async run(t) {
      const st = await openRow(t);
      await t.shot('private-apps-list', S_CLIP);
      const p = await facts(st);
      const names = APPS.map((a) => `${a.name}|${a.exe}`).join();
      const labels = APPS.map((a) => `Remove ${a.name}`).join();
      const ok = p.open && p.summary === '7 apps' && p.apps.join() === names && p.removes.join() === labels && p.listShown && !p.empty && !p.frontOff && p.frontNote === 'Notepad (notepad.exe)' && p.wide === 0;
      t.check('privateAppsList', ok, JSON.stringify(p));

      await answer(t, 'privacy:remove', { apps: APPS.slice(1), front: front(false), removed: true });
      await st.click('#private-list button[aria-label="Remove 1Password"]');
      await t.settle(300);
      const sent = await payloads(t, 'privacy:remove');
      const q = await facts(st);
      // The next app's button takes the keyboard where the removed one was.
      const gone = sent.length === 1 && sent[0].exe === '1password.exe' && q.summary === '6 apps' && q.apps.length === 6 && !q.apps.join().includes('1Password') && q.status === '1Password was taken off the list.' && !q.error && q.focus === 'Remove Bitwarden';
      t.check('privateAppsRemove', gone, JSON.stringify({ sent, q }));
      await t.shot('private-apps-removed', S_CLIP);
    },
  },

  // Adding an app by the name of its program.
  {
    name: 'private-apps-adding',
    backdrop: 'wallpaper',
    windows: [settingsWin()],
    async run(t) {
      const st = await openRow(t);
      await st.fill('#private-name', 'Notepad');
      await t.shot('private-apps-typing', S_CLIP);
      await answer(t, 'privacy:add', { ...WITH_NOTEPAD, added: { ...NOTEPAD, new: true } });
      await st.press('#private-name', 'Enter');
      await t.settle(300);
      const sent = await payloads(t, 'privacy:add');
      const p = await facts(st);
      const ok = sent.length === 1 && sent[0].name === 'Notepad' && p.summary === '8 apps' && p.apps.includes('Notepad|notepad.exe') && p.field === '' && p.status === 'Notepad is on the list. Aevaleron will not read it.' && !p.error && p.focus === 'private-name' && p.frontOff && p.frontNote === 'Notepad is already on the list.';
      t.check('privateAppsAddByName', ok, JSON.stringify({ sent, p }));
      await st.evaluate(() => document.getElementById('private-status').scrollIntoView({ block: 'end' }));
      await t.shot('private-apps-added', S_CLIP);

      // A name already on the list says so and changes nothing.
      await answer(t, 'privacy:add', { ...WITH_NOTEPAD, added: { ...NOTEPAD, new: false } });
      await st.fill('#private-name', 'notepad.exe');
      await st.click('#private-add');
      await t.settle(300);
      const again = await facts(st);
      t.check('privateAppsAddTwice', again.summary === '8 apps' && again.status === 'Notepad is already on the list.', JSON.stringify(again));
    },
  },

  // "Add the app that was in front", then a name that is not one.
  {
    name: 'private-apps-front',
    backdrop: 'wallpaper',
    windows: [settingsWin()],
    async run(t) {
      const st = await openRow(t);
      await answer(t, 'privacy:add', { ...WITH_NOTEPAD, added: { ...NOTEPAD, new: true } });
      await st.click('#private-add-front');
      await t.settle(300);
      const sent = await payloads(t, 'privacy:add');
      const p = await facts(st);
      // The button is off once its app is listed, so the field takes over.
      const ok = sent.length === 1 && sent[0].front === true && p.apps.includes('Notepad|notepad.exe') && p.status === 'Notepad is on the list. Aevaleron will not read it.' && p.frontOff && p.focus === 'private-name';
      t.check('privateAppsAddFront', ok, JSON.stringify({ sent, p }));

      // Nothing typed: said on the page, and Rust is not asked.
      await st.click('#private-add');
      await t.settle(200);
      const none = await facts(st);
      const asked = (await payloads(t, 'privacy:add')).length;
      t.check('privateAppsAddNothing', asked === 1 && none.error && /^Type the name of the app's program first/.test(none.status) && none.focus === 'private-name', JSON.stringify({ asked, none }));

      await answer(t, 'privacy:add', { ...WITH_NOTEPAD, error: BAD_NAME });
      await st.fill('#private-name', 'a*b');
      await st.click('#private-add');
      await t.settle(300);
      const bad = await facts(st);
      t.check('privateAppsBadName', bad.error && bad.status === BAD_NAME && bad.field === 'a*b' && bad.focus === 'private-name' && bad.summary === '8 apps', JSON.stringify(bad));
      await st.evaluate(() => document.getElementById('private-status').scrollIntoView({ block: 'end' }));
      await t.shot('private-apps-error', S_CLIP);
    },
  },

  // The bar over a listed app: one plain line, and the way to the list.
  {
    name: 'private-apps-bar',
    backdrop: 'wallpaper',
    textScale: true,
    windows: [overlay],
    async run(t) {
      const f = t.frame('overlay');
      await f.evaluate(() => localStorage.setItem('aeval.hasSentFirstRequest', '1'));
      await openBar(f, { privateNotice: LINE });
      await t.settle();
      await t.shot('private-apps-bar', BAR_CLIP);
      const says = () =>
        f.evaluate(() => ({
          shown: !document.getElementById('notice').hidden,
          text: document.getElementById('notice-text').textContent,
          button: document.getElementById('notice-settings').hidden ? '' : document.getElementById('notice-settings').textContent,
        }));
      const n = await says();
      t.check('privateAppsBarNotice', n.shown && n.text === LINE && n.button === 'Private apps', JSON.stringify(n));
      await f.click('#notice-settings');
      const opened = (await t.calls('overlay', 'overlay:open-settings')).map((c) => c.payload);
      t.check('privateAppsBarOpensTheList', opened.length === 1 && opened[0] && opened[0].section === 'sec-private', JSON.stringify(opened));
      // It is said before a missing provider is: the promise comes first.
      await openBar(f, { privateNotice: LINE, hasKey: false });
      await t.settle(200);
      const first = await says();
      // Another app in front again: the line goes.
      await openBar(f, { privateNotice: '' });
      await t.settle(200);
      const after = await says();
      t.check('privateAppsBarNoticeComesAndGoes', first.text === LINE && !after.shown, JSON.stringify({ first, after }));
    },
  },

  // "What was sent" for a question asked over a listed app.
  {
    name: 'private-apps-sent',
    backdrop: 'wallpaper',
    windows: [{ name: 'out1', src: '/renderer/output.html', x: OUT1.x, y: OUT1.y, w: OUT1.w, h: OUT1.h, z: 2, max: 900 }],
    async run(t) {
      const f = t.frame('out1');
      const sent = { provider: 'openrouter', providerName: 'OpenRouter', model: 'google/gemma-3-27b-it:free', modelName: 'Gemma 3 27B', image: null, history: 0, free: false, private: LINE };
      await f.evaluate((s) => {
        const usage = { model: s.model, provider: s.provider, ttftMs: 900, ms: 3200, prompt: 300, completion: 120, cost: 0 };
        const q = 'How long should a master password be?';
        window.__emit('overlay:state', { mode: 'panel', chatMax: 640 });
        window.__emit('chat:user', { gen: 1, text: q });
        window.__emit('query:start', { gen: 1 });
        window.__emit('query:delta', { gen: 1, text: 'Four or five unrelated words are easier to remember and harder to guess than a short string of symbols.' });
        window.__emit('query:done', { gen: 1, usage, question: q, sent: s, msgId: 3 });
      }, sent);
      await t.settle(700);
      await f.evaluate(() => document.querySelector('.sent-btn').click());
      await t.settle(300);
      await t.shot('private-apps-sent', await clipOf(t, 'out1'));
      const p = await f.evaluate(() => {
        const btn = document.querySelector('.sent-btn');
        const panel = document.getElementById(btn.getAttribute('aria-controls'));
        return { shown: !panel.hidden, text: panel.innerText };
      });
      const ok = p.shown && p.text.includes('Your question') && p.text.includes(`Nothing from the screen. ${LINE}`) && !/screenshot|The text of/.test(p.text);
      t.check('privateAppsSentSaysNothingWasRead', ok, JSON.stringify(p));
    },
  },
];
