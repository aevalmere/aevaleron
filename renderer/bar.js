'use strict';

// The single input. One field takes a question or a task, and the token at
// its left says which one Enter will do. The guess comes from
// renderer/intent.js as the person types; a prefix (? or >), Ctrl+1 or
// Ctrl+2, or the token's menu (Alt+Down) settles it outright, and Alt+Enter
// does the other one. A clear task runs on Enter through the same
// agent:start a > sends; a line that is not clearly either is asked, and the
// answer offers to do it instead.
//
// Loaded after overlay.js, which owns the window state, the notice and the
// resize; both are classic scripts sharing one global scope.
//
// The bar is the app's only text input (REQUESTS.md 9). A follow-up goes to
// the focused or most recent output window (Rust routes query:submit), and
// when a task asks a question or waits on its plan, Enter here answers it.

const Intent = window.AevalIntent;
const token = document.getElementById('intent');
const tokenName = token.querySelector('.intent-name');
const intentDesc = document.getElementById('intent-desc');
const intentMenu = document.getElementById('intent-menu');
const examplesEl = document.getElementById('examples');
const announcer = document.getElementById('announce');
const targetChip = document.getElementById('target-chip');
const targetText = document.getElementById('target-text');
const targetNew = document.getElementById('target-new');
const targetDesc = document.getElementById('target-desc');
const modelChip = document.getElementById('model-chip');
const modelChipName = document.getElementById('model-chip-name');
const modelMenu = document.getElementById('model-menu');
const modelList = document.getElementById('model-list');

const PREFIX = { ask: '?', do: '>' };
const ENTER_SAYS = {
  ask: 'Enter asks',
  do: 'Enter does it on your computer',
};
const KEYS_SAY = 'Ctrl+1 asks, Ctrl+2 does it as a task, Alt+Enter does the other one.';
// Short enough to sit beside the sigil legend in a 420px capsule at 18px;
// the field's accessible name keeps the full sentence.
const PLACEHOLDER = 'Ask or do';

// lock: null while guessing, else the intent the person chose.
const bar = {
  lock: null,
  guess: Intent.classify(''),
  history: [],
  recall: -1,
  draft: '',
  announced: '',
  announceTimer: null,
  // The target's label as last shown, and whether Ctrl+Tab is moving it.
  targetLabel: '',
  cycling: false,
  // After Enter met "needs a provider", Tab goes to the notice's button.
  tabToNotice: false,
  // The model chosen with @ for this one question, null for the usual one.
  pin: null,
  // Every ready provider's models, asked for once per opening of the bar.
  models: null,
  modelRows: [],
  // Esc closed the list for this @, so the rest is plain text.
  modelDismissed: false,
};
let barHint = '';

// ------------------------------------------------------------------- rows

// The field grows with its text up to seven rows, then scrolls; the bar's
// glass grows with it and overlay.js's ResizeObserver reports the new
// height (overlay:resize, capped by Rust's INPUT_MAX_HEIGHT).
const MAX_ROWS = 7;
let fieldWidth = 0;

function fieldMetrics() {
  const cs = getComputedStyle(input);
  return {
    lh: parseFloat(cs.lineHeight) || 24,
    pad: (parseFloat(cs.paddingTop) || 0) + (parseFloat(cs.paddingBottom) || 0),
  };
}

function fitField() {
  // Hidden with the panel: nothing to measure. Opening refits it.
  if (!input.clientWidth) return;
  const { lh, pad } = fieldMetrics();
  input.style.height = 'auto';
  const need = input.scrollHeight;
  const max = Math.round(lh * MAX_ROWS + pad);
  input.style.height = `${Math.min(need, max)}px`;
  input.style.overflowY = need > max + 1 ? 'auto' : 'hidden';
  barEl.dataset.rows = String(Math.max(1, Math.min(MAX_ROWS, Math.round((need - pad) / lh))));
}

// A twin of the field, off screen, answers "how many rows would this text
// take at the field's width?", which tells whether the caret is on the
// first or the last row, wrapped lines included.
let mirror = null;
function rowsOf(text) {
  if (!mirror) {
    mirror = document.createElement('textarea');
    mirror.className = 'query-mirror';
    mirror.rows = 1;
    mirror.tabIndex = -1;
    mirror.setAttribute('aria-hidden', 'true');
    document.body.appendChild(mirror);
  }
  mirror.style.width = `${input.clientWidth}px`;
  mirror.value = text;
  const { lh, pad } = fieldMetrics();
  return Math.max(1, Math.round((mirror.scrollHeight - pad) / lh));
}

// Up recalls only from the first row and Down only from the last, so the
// arrows move through a long question before they reach the history.
function caretAtEdge(dir) {
  const v = input.value;
  if (input.selectionStart !== input.selectionEnd) return false;
  if (!v.includes('\n') && rowsOf(v) === 1) return true;
  const at = input.selectionStart;
  return dir < 0 ? rowsOf(v.slice(0, at)) === 1 : rowsOf(v.slice(0, at)) === rowsOf(v);
}

new ResizeObserver(() => {
  if (input.clientWidth === fieldWidth) return;
  fieldWidth = input.clientWidth;
  fitField();
}).observe(input);
window.addEventListener('aeval:text-scale', () => requestAnimationFrame(fitField));

// ------------------------------------------------------------- the guess

function barIntent() {
  return bar.lock || bar.guess.intent;
}

function classifyNow() {
  bar.guess = Intent.score(input.value);
  renderToken();
}

// The token's width follows its word: measured before and after the change
// and animated between the two (FLIP), so the capsule morphs instead of
// snapping. The field beside it never moves; the token has room reserved.
let tokenFlip = null;
function flipToken(change) {
  const w0 = token.getBoundingClientRect().width;
  change();
  const w1 = token.getBoundingClientRect().width;
  if (document.documentElement.dataset.motion === 'reduced' || !w0 || Math.abs(w1 - w0) < 1) return;
  clearTimeout(tokenFlip);
  token.style.transition = 'none';
  token.style.width = `${w0}px`;
  void token.offsetWidth;
  token.style.transition = 'width 240ms var(--ease-morph), background 160ms ease, box-shadow 160ms ease';
  token.style.width = `${w1}px`;
  tokenFlip = setTimeout(() => {
    token.style.width = '';
    token.style.transition = '';
  }, 260);
}

// While a task waits on the person, Enter answers it unless they chose an
// intent outright: asking takes the typed reply, a plan or a pause takes an
// empty Enter. The token then says so (Reply, Start, Continue).
const WAIT_NAME = { asking: 'Reply', plan: 'Start', paused: 'Continue' };
function replyState() {
  if (bar.lock) return '';
  const w = agentWaiting();
  if (w === 'asking') return w;
  if ((w === 'plan' || w === 'paused') && !input.value.trim()) return w;
  return '';
}

function renderToken() {
  fitField();
  const waiting = replyState();
  const now = waiting ? 'do' : barIntent();
  const name = waiting ? WAIT_NAME[waiting] : Intent.NAME[now];
  const locked = !!bar.lock || !!waiting;
  // At rest the token is only its glyph: nothing is chosen, and Ask, the
  // default, needs no word. Do, a choice or a reply shows the word.
  const rest = locked || now !== 'ask' ? 'false' : 'true';
  const wasLocked = token.dataset.locked === 'true';
  if (tokenName.textContent !== name || token.dataset.intent !== now || token.dataset.rest !== rest) {
    flipToken(() => {
      token.dataset.intent = now;
      token.dataset.rest = rest;
      tokenName.textContent = name;
    });
    replayAnim(token, 'swap');
  }
  token.dataset.locked = locked ? 'true' : 'false';
  // Low when the line could be either: it is asked, and the answer offers
  // to do it instead.
  const unsure = !locked && bar.guess.offerDo && !!input.value.trim();
  token.dataset.confidence = unsure ? 'low' : 'high';
  if (locked && !wasLocked) replayAnim(token, 'lock');
  if (waiting) {
    token.setAttribute('aria-label', `${name}. Enter sends this to the running task.`);
    token.setAttribute('aria-disabled', 'true');
    intentDesc.textContent = `Enter sends this to the running task. ${KEYS_SAY}`;
  } else {
    const how = bar.lock ? 'chosen' : 'guessed';
    token.setAttribute('aria-label', `${Intent.NAME[now]}, ${how}. Choose what Enter does.`);
    token.removeAttribute('aria-disabled');
    const offer = unsure ? ', and the answer offers to do it instead' : '';
    intentDesc.textContent = `${ENTER_SAYS[now]}${offer}. ${KEYS_SAY}`;
  }
  // Said once the typing settles, and only when it changed; announcing every
  // keystroke's guess would talk over the person typing.
  clearTimeout(bar.announceTimer);
  bar.announceTimer = setTimeout(() => {
    const w = replyState();
    const say = w ? WAIT_NAME[w] : `${Intent.NAME[barIntent()]}${bar.lock ? '' : ', guessed'}`;
    if (say !== bar.announced && input.value.trim()) {
      bar.announced = say;
      announce(say);
    }
  }, 700);
  renderStatus();
  renderNotice();
  renderTarget();
  renderPin();
  sendDirty();
}

// Rust's global Esc stops a waiting task; when the bar holds a half-typed
// answer it clears that first (U17). It needs to know whether the field
// has text: sent when that changes, 100 ms after the typing.
let dirtySent = false;
let dirtyTimer = null;
function sendDirty() {
  clearTimeout(dirtyTimer);
  dirtyTimer = setTimeout(() => {
    const dirty = !!input.value;
    if (dirty === dirtySent) return;
    dirtySent = dirty;
    window.aeval.invoke('bar:dirty', { dirty }).catch(() => {});
  }, 100);
}

// Listening to a channel Rust has not ported yet throws; the bar works
// without it.
function listenSoft(channel, fn) {
  try {
    window.aeval.on(channel, fn);
  } catch (err) {
    // Not in the bridge's list yet.
  }
}

listenSoft('bar:clear', () => {
  clearField();
  input.focus();
});

// A short note in the status row, for a few seconds: from Rust (the window
// cap), or why Enter on a match did nothing. The longer ones stay longer.
let barNote = '';
let noteTimer = null;
function showNote(text) {
  barNote = String(text || '');
  clearTimeout(noteTimer);
  noteTimer = setTimeout(() => {
    barNote = '';
    renderStatus();
  }, barNote.length > 60 ? 7000 : 4000);
  renderStatus();
}
listenSoft('bar:note', (p) => showNote(p && p.text));

// ------------------------------------------------------------- the target

// With two or more answer windows open, the bar says which one the next
// question continues (overlay:state.target and visibleOutputs, from Rust).
// One window, or none: no chip.
function renderTarget() {
  const t = state.target;
  const title = t && t.title ? String(t.title) : '';
  const show = !!title && Number(state.visibleOutputs || 0) >= 2 && !replyState();
  const label = show ? String(t.label || title) : '';
  if (show) {
    const short = title.length > 28 ? `${title.slice(0, 27).trimEnd()}…` : title;
    targetText.textContent = `Continues: ${short}`;
    targetChip.title = `Continues: ${title}`;
    targetDesc.textContent = `Continues the conversation "${title}".`;
    // After Ctrl+Tab, the new target is said once Rust has moved it.
    if (bar.cycling && label !== bar.targetLabel) {
      bar.cycling = false;
      announce(`Continues "${title}"`);
    }
  } else {
    targetDesc.textContent = '';
  }
  bar.targetLabel = label;
  if (targetChip.hidden === !show) return;
  targetChip.hidden = !show;
  requestResize();
}

targetNew.addEventListener('click', () => {
  newConversation();
  input.focus();
});

// Ctrl+Tab and Ctrl+Shift+Tab move the target through the open windows;
// Rust answers with the new target in overlay:state.
function cycleTarget(dir) {
  if (Number(state.visibleOutputs || 0) < 2) return;
  bar.cycling = true;
  window.aeval.invoke('output:cycle-target', { dir }).catch(() => {
    bar.cycling = false;
  });
}

// ---------------------------------------------------- one question's model

// @ as the first character chooses the model for this one question (U20).
// Focus stays in the field, so typing goes on narrowing the list; the pick
// becomes a chip after the token and goes with the question.
function providerModels(r) {
  const out = [];
  for (const p of (r && r.providers) || []) {
    const seen = new Set();
    const models = [];
    for (const g of p.groups || []) {
      for (const m of g.models || []) {
        if (seen.has(m.id)) continue;
        seen.add(m.id);
        models.push({ id: m.id, name: m.name || m.id });
      }
    }
    if (models.length) out.push({ id: p.id, name: p.name, models });
  }
  return out;
}

let modelsAsked = false;
function askForModels() {
  if (modelsAsked) return;
  modelsAsked = true;
  invokeSoft('models:list', {}).then((r) => {
    modelsAsked = false;
    bar.models = providerModels(r);
    syncModelMenu();
  });
}

// Open, and not on its way out (the shared 120 ms fade, glass.js).
const modelMenuOpen = () => !modelMenu.hidden && !modelMenu.classList.contains('closing');

function closeModelMenu() {
  if (!modelMenuOpen()) return;
  window.AevalGlass.menuOut(modelMenu, () => {
    modelMenu.hidden = true;
    requestResize();
  });
  input.removeAttribute('aria-activedescendant');
  input.removeAttribute('aria-controls');
  input.removeAttribute('aria-autocomplete');
}

function markModel(at) {
  bar.modelRows.forEach((row, i) => row.el.setAttribute('aria-selected', i === at ? 'true' : 'false'));
  if (at < 0) {
    input.removeAttribute('aria-activedescendant');
    return;
  }
  input.setAttribute('aria-activedescendant', bar.modelRows[at].el.id);
  bar.modelRows[at].el.scrollIntoView({ block: 'nearest' });
}

function markedModel() {
  return bar.modelRows.findIndex((row) => row.el.getAttribute('aria-selected') === 'true');
}

function modelOption(row, n) {
  const el = document.createElement('div');
  el.id = `model-opt-${n}`;
  el.setAttribute('role', 'option');
  const label = document.createElement('span');
  label.className = 'g-menu-label';
  label.textContent = row.name;
  el.appendChild(label);
  el.addEventListener('click', () => pickModel(row));
  row.el = el;
  return el;
}

function renderModelMenu(filter) {
  const needle = filter.toLowerCase();
  const rows = [];
  modelList.textContent = '';
  for (const p of bar.models) {
    const hits = p.models.filter((m) => `${m.name} ${m.id}`.toLowerCase().includes(needle));
    if (!hits.length) continue;
    const group = document.createElement('div');
    group.setAttribute('role', 'group');
    group.setAttribute('aria-label', p.name);
    const head = document.createElement('div');
    head.className = 'g-menu-group-label';
    head.setAttribute('aria-hidden', 'true');
    head.textContent = p.name;
    group.appendChild(head);
    for (const m of hits) {
      const row = { provider: p.id, providerName: p.name, model: m.id, name: m.name };
      group.appendChild(modelOption(row, rows.length));
      rows.push(row);
    }
    modelList.appendChild(group);
  }
  bar.modelRows = rows;
  if (!rows.length) {
    const none = document.createElement('div');
    none.className = 'g-menu-empty';
    none.textContent = `No model called "${filter}". Esc to keep typing.`;
    modelList.appendChild(none);
    announceSoon(none.textContent);
  }
  window.AevalGlass.menuIn(modelMenu);
  modelMenu.hidden = false;
  input.setAttribute('aria-controls', 'model-menu');
  input.setAttribute('aria-autocomplete', 'list');
  markModel(rows.length ? 0 : -1);
  requestResize();
}

// Called on every change to the field. A provider with no models, or none
// ready, leaves the @ as ordinary text.
function syncModelMenu() {
  const v = input.value;
  const typed = v.startsWith('@');
  if (!typed) bar.modelDismissed = false;
  if (!typed || bar.modelDismissed || bar.pin || replyState()) {
    closeModelMenu();
    return;
  }
  if (!bar.models) {
    askForModels();
    return;
  }
  if (bar.models.length) renderModelMenu(v.slice(1).trim());
  else closeModelMenu();
}

function pickModel(row) {
  bar.pin = { provider: row.provider, providerName: row.providerName, model: row.model, name: row.name };
  input.value = '';
  closeModelMenu();
  classifyNow();
  renderExamplesShown();
  announce(`${row.name} for this question`);
  input.focus();
}

function clearPin(say) {
  if (!bar.pin) return;
  bar.pin = null;
  renderPin();
  if (say) announce('Model choice removed');
}

function renderPin() {
  const p = bar.pin;
  if (p) {
    modelChipName.textContent = p.name;
    modelChip.setAttribute('aria-label', `Model for this question: ${p.name} via ${p.providerName}. Press to remove.`);
  }
  if (modelChip.hidden === !p) return;
  modelChip.hidden = !p;
  requestResize();
}

// True when the list took the key.
function modelKey(e) {
  if (!modelMenuOpen() || e.ctrlKey || e.altKey || e.metaKey) return false;
  const rows = bar.modelRows;
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    e.preventDefault();
    if (rows.length) markModel((markedModel() + (e.key === 'ArrowDown' ? 1 : -1) + rows.length) % rows.length);
    return true;
  }
  if (e.key === 'Enter' || (e.key === 'Tab' && !e.shiftKey)) {
    e.preventDefault();
    if (rows.length) pickModel(rows[Math.max(0, markedModel())]);
    return true;
  }
  if (e.key === 'Escape') {
    e.preventDefault();
    e.stopPropagation();
    bar.modelDismissed = true;
    closeModelMenu();
    return true;
  }
  return false;
}

// The field keeps focus through a click on the list.
modelMenu.addEventListener('pointerdown', (e) => e.preventDefault());
input.addEventListener('blur', closeModelMenu);
modelChip.addEventListener('click', () => {
  clearPin(true);
  input.focus();
});

function setLock(next, opts) {
  const o = opts || {};
  bar.lock = next && Intent.INTENTS.includes(next) ? next : null;
  renderToken();
  if (o.say) announce(bar.lock ? `${Intent.NAME[bar.lock]}, chosen` : 'Guessing again');
}

// A prefix typed at the start becomes the lock and leaves the text.
function takePrefix() {
  if (bar.lock) return;
  const v = input.value;
  const lock = Intent.parse(v).lock;
  if (!lock) return;
  const at = Math.max(0, input.selectionStart - 1);
  input.value = v.slice(1).replace(/^\s/, '');
  input.setSelectionRange(at, at);
  setLock(lock, { say: true });
}

// ------------------------------------------------------------- the menu

// Open, and not on its way out (the shared 120 ms fade, glass.js).
const menuOpen = () => !intentMenu.hidden && !intentMenu.classList.contains('closing');

function openMenu() {
  // Enter belongs to the running task; Ctrl+1, Ctrl+2 or a prefix still choose.
  if (replyState()) return;
  window.AevalGlass.menuIn(intentMenu);
  intentMenu.hidden = false;
  token.setAttribute('aria-expanded', 'true');
  const opts = [...intentMenu.querySelectorAll('[role="option"]')];
  for (const o of opts) o.setAttribute('aria-selected', o.dataset.intent === barIntent() ? 'true' : 'false');
  const cur = opts.find((o) => o.dataset.intent === barIntent()) || opts[0];
  cur.focus();
  requestResize();
}

function closeMenu(refocus) {
  if (!menuOpen()) return;
  window.AevalGlass.menuOut(intentMenu, () => {
    intentMenu.hidden = true;
    requestResize();
  });
  token.setAttribute('aria-expanded', 'false');
  if (refocus) input.focus();
}

token.addEventListener('click', () => {
  hideTip();
  if (menuOpen()) closeMenu(true);
  else openMenu();
});

// The token's key hints as a glass row under it (U44): after 600 ms of
// keyboard focus or 400 ms of hover. While it shows on hover, the system
// tooltip, which says the same, steps aside.
const tokenTip = document.getElementById('token-tip');
let tipTimer = null;
function showTip(delay) {
  clearTimeout(tipTimer);
  tipTimer = setTimeout(() => {
    if (menuOpen() || !tokenTip.hidden) return;
    tokenTip.hidden = false;
    requestResize();
  }, delay);
}
function hideTip() {
  clearTimeout(tipTimer);
  if (tokenTip.hidden) return;
  tokenTip.hidden = true;
  requestResize();
}
token.addEventListener('pointerenter', () => {
  if (token.title) {
    token.dataset.title = token.title;
    token.removeAttribute('title');
  }
  showTip(400);
});
token.addEventListener('pointerleave', () => {
  if (token.dataset.title) {
    token.title = token.dataset.title;
    delete token.dataset.title;
  }
  if (!token.matches(':focus-visible')) hideTip();
});
token.addEventListener('focus', () => {
  if (token.matches(':focus-visible')) showTip(600);
});
token.addEventListener('blur', hideTip);
token.addEventListener('keydown', (e) => {
  if (e.key !== 'Tab') hideTip();
});
intentMenu.addEventListener('click', (e) => {
  const o = e.target.closest('[role="option"]');
  if (!o) return;
  setLock(o.dataset.intent, { say: true });
  closeMenu(true);
});
intentMenu.addEventListener('keydown', (e) => {
  const opts = [...intentMenu.querySelectorAll('[role="option"]')];
  const i = opts.indexOf(document.activeElement);
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    e.preventDefault();
    opts[(i + (e.key === 'ArrowDown' ? 1 : -1) + opts.length) % opts.length].focus();
  } else if (e.key === 'Enter' || e.key === ' ') {
    e.preventDefault();
    if (i >= 0) setLock(opts[i].dataset.intent, { say: true });
    closeMenu(true);
  } else if (e.key === 'Escape') {
    e.preventDefault();
    e.stopPropagation();
    closeMenu(true);
  } else if (e.key === 'Tab') {
    closeMenu(false);
  }
});

// ----------------------------------------------------------------- status

// One line under the bar: a note from Rust, or why Enter did nothing.
function renderStatus() {
  let text = '';
  if (barNote) text = barNote;
  if (barHint) text = barHint;
  if (barStatusText.textContent !== text && text) announceSoon(text);
  barStatusText.textContent = text;
  const show = !!text;
  if (barStatus.hidden === show) barStatus.hidden = !show;
  requestResize();
}

// --------------------------------------------------------------- examples

// Starting points from the app in front, under an empty bar. Each one fills
// the field, so it also shows what the guess will make of it.
function renderExamples(list) {
  examplesEl.textContent = '';
  for (const ex of list.slice(0, 3)) {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'example-chip glass thin interactive';
    b.textContent = ex;
    b.addEventListener('click', () => {
      input.value = ex;
      setLock(null);
      input.focus();
      onInput();
    });
    examplesEl.appendChild(b);
  }
  renderExamplesShown();
}

// Suggestions are for someone who has not used the bar yet: shown until the
// first request is sent, then never again (REQUESTS.md 3). Rust keeps the
// flag in the settings file (hasSentFirstRequest, sent in overlay:state);
// localStorage holds it too, so the chips go at once, before Rust answers.
const FIRST_KEY = 'aeval.hasSentFirstRequest';
function sentBefore() {
  if (state.hasSentFirstRequest) return true;
  try {
    return localStorage.getItem(FIRST_KEY) === '1';
  } catch (err) {
    return false;
  }
}

function markSent() {
  if (sentBefore()) return;
  try {
    localStorage.setItem(FIRST_KEY, '1');
  } catch (err) {
    // Rust's copy of the flag still lands with the request.
  }
  state.hasSentFirstRequest = true;
  examplesEl.textContent = '';
  renderExamplesShown();
}

function renderExamplesShown() {
  const show = !sentBefore() && !input.value && examplesEl.childElementCount > 0;
  if (examplesEl.hidden === !show) return;
  examplesEl.hidden = !show;
  requestResize();
}

window.aeval.on('bar:context', (c) => {
  if (!sentBefore()) renderExamples(Intent.examples(c && c.process, c && c.title));
});

// ---------------------------------------------------------------- sending

function pushHistory(text) {
  const t = String(text || '').trim();
  if (!t) return;
  window.aeval.invoke('bar:history-push', { text: t }).then((h) => {
    if (Array.isArray(h)) bar.history = h;
  });
  bar.recall = -1;
}

function clearField() {
  input.value = '';
  bar.pin = null;
  setLock(null);
  renderExamplesShown();
}

// what: the intent to send as. A question the bar was unsure of carries
// offerDo, so its answer offers to do it instead.
function send(what, offerDo) {
  const text = input.value;
  const body = text.trim();
  // Read before clearField, which gives the choice back.
  const pin = bar.pin ? { provider: bar.pin.provider, model: bar.pin.model } : {};
  barHint = '';
  if (body) markSent();
  const recallAs = bar.lock ? `${PREFIX[bar.lock]}${body}` : body;
  if (what === 'do') {
    if (!body) {
      barHint = 'Say what to do, for example "rename these files by date".';
      renderStatus();
      return;
    }
    pushHistory(`>${body}`);
    window.aeval.invoke('agent:start', { task: body, ...pin }).then((r) => {
      // Waiting on consent keeps the words: declining must not cost them.
      if (r && (r.status === 'started' || r.status === 'offline')) clearField();
      else if (r && r.status === 'busy') barHint = 'A task is already running. Esc stops it.';
      else if (r && r.status === 'unsupported') barHint = 'Doing tasks needs Windows.';
      else if (r && r.status === 'needs-key') barHint = '';
      renderStatus();
      renderNotice();
      if (r && r.status === 'needs-key') needsKey('do');
    }, () => {
      barHint = "Couldn't start the task. Press Enter to try again.";
      renderStatus();
    });
    return;
  }
  pushHistory(recallAs);
  // Cleared at once, like a messaging app; the transcript echoes it. With
  // no key the text stays so nothing typed is lost.
  if (state.hasKey) clearField();
  // A question that never started gives its words back, and says so.
  const unsent = () => {
    if (!input.value) input.value = text;
    barHint = "Couldn't send that. Press Enter to try again.";
    renderStatus();
  };
  window.aeval.invoke('query:submit', { question: body, offerDo: !!offerDo, ...pin }).then((result) => {
    if (result && result.status === 'walkthrough') clearField();
    else if (result && result.status === 'needs-key') needsKey('ask');
    else if (result && result.status === 'error') unsent();
  }, unsent);
}

// Enter with no provider connected: the notice that says so nudges, the
// reason is said at once, and Tab from the field goes straight to its
// button (U23).
function needsKey(what) {
  renderNotice();
  if (notice.hidden) return;
  replayAnim(notice, 'nudge');
  announce(`${what === 'do' ? 'Tasks' : 'Answers'} need an AI provider. Press Tab to connect one.`, 'assertive');
  bar.tabToNotice = true;
}

// A run that is waiting on the person: a question (asking), its plan
// (plan) or a pause. Rust says so in overlay:state.agent.
function agentWaiting() {
  const a = state.agent;
  if (!a || !a.running) return '';
  return a.state === 'asking' || a.state === 'plan' || a.state === 'paused' ? a.state : '';
}

const WAITING_PLACEHOLDER = {
  asking: 'Type your answer for the task',
  plan: 'Enter starts the task',
  paused: 'Enter continues the task',
};

// Called by overlay.js whenever the state changes.
function syncBarFromState() {
  const w = agentWaiting();
  const was = document.body.dataset.waiting || '';
  input.placeholder = WAITING_PLACEHOLDER[w] || PLACEHOLDER;
  input.setAttribute('aria-label', WAITING_PLACEHOLDER[w] || 'Ask, or tell me what to do');
  document.body.dataset.waiting = w || '';
  // A task that starts asking while someone is in the bar: say where the
  // answer goes, at once. Elsewhere the trail says it, so only here.
  if (w === 'asking' && was !== 'asking' && state.mode === 'panel' && document.hasFocus()) {
    announce('The task has a question. Type your answer here and press Enter.', 'assertive');
  }
  renderExamplesShown();
  // The token tells the truth about Enter as the task moves (Reply, Start,
  // Continue); it also re-renders the target chip.
  if (w !== was) renderToken();
  else renderTarget();
}

function onEnter(e) {
  // A task waiting on the person gets Enter, unless they chose an intent
  // outright (a prefix, Ctrl+1 or Ctrl+2, the menu), which is honoured.
  const waiting = replyState();
  const typed = input.value.trim();
  if (waiting === 'asking') {
    if (!typed) return;
    pushHistory(typed);
    window.aeval.invoke('agent:continue', { text: typed });
    clearField();
    return;
  }
  if (waiting === 'plan' || waiting === 'paused') {
    window.aeval.invoke('agent:continue', {});
    return;
  }
  const now = barIntent();
  // Alt+Enter does the other one: a question the bar would ask runs as a
  // task, a task is only answered.
  if (e.altKey) {
    send(now === 'do' ? 'ask' : 'do');
    return;
  }
  if (now === 'do') {
    send('do');
    return;
  }
  send('ask', !bar.lock && bar.guess.offerDo);
}

// ---------------------------------------------------------------- keyboard

function onInput() {
  barHint = '';
  bar.tabToNotice = false;
  if (bar.recall >= 0 && input.value !== bar.history[bar.recall]) bar.recall = -1;
  takePrefix();
  classifyNow();
  renderExamplesShown();
  syncModelMenu();
}

input.placeholder = PLACEHOLDER;
input.addEventListener('input', onInput);

// Ctrl+1 chooses Ask and Ctrl+2 Do. Read from the physical key as well as
// the character, so a layout that puts something else on the digit row
// still works.
function chordLock(e) {
  if (!e.ctrlKey || e.altKey || e.shiftKey || e.metaKey) return false;
  const digit = /^(Digit|Numpad)[12]$/.test(e.code || '') ? e.code.slice(-1) : e.key;
  const lock = digit === '1' || digit === '2' ? Intent.INTENTS[Number(digit) - 1] : null;
  if (!lock) return false;
  e.preventDefault();
  setLock(lock, { say: true });
  return true;
}

// The same chords with the focus anywhere else in the bar (the token, a
// button, nothing at all): they choose, and the field takes the typing.
document.addEventListener('keydown', (e) => {
  if (e.target === input || e.isComposing || e.keyCode === 229) return;
  if (chordLock(e)) input.focus();
});

input.addEventListener('keydown', (e) => {
  // An IME is still composing: Enter picks the candidate, not the action.
  if (e.isComposing || e.keyCode === 229) return;
  if (modelKey(e)) return;
  if (chordLock(e)) return;
  if (e.altKey && e.key === 'ArrowDown') {
    e.preventDefault();
    openMenu();
    return;
  }
  if (e.key === 'F6') {
    e.preventDefault();
    window.aeval.invoke('ui:focus-answer');
    return;
  }
  // Backspace at the start gives back what sits before the text: the model
  // first, then the lock.
  if (e.key === 'Backspace' && bar.pin && input.selectionStart === 0 && input.selectionEnd === 0) {
    e.preventDefault();
    clearPin(true);
    return;
  }
  if (e.key === 'Backspace' && bar.lock && input.selectionStart === 0 && input.selectionEnd === 0) {
    e.preventDefault();
    setLock(null, { say: true });
    return;
  }
  if (e.key === 'Tab' && e.ctrlKey && !e.altKey) {
    e.preventDefault();
    cycleTarget(e.shiftKey ? -1 : 1);
    return;
  }
  if (e.key === 'Tab' && !e.shiftKey && !e.altKey && bar.tabToNotice && !notice.hidden && !noticeSettings.hidden) {
    e.preventDefault();
    bar.tabToNotice = false;
    noticeSettings.focus();
    return;
  }
  if ((e.key === 'ArrowUp' || e.key === 'ArrowDown') && !e.altKey && !e.shiftKey && !e.ctrlKey) {
    const dir = e.key === 'ArrowUp' ? -1 : 1;
    if (caretAtEdge(dir) && recallKey(dir)) e.preventDefault();
    return;
  }
  if (e.key === 'Escape') {
    e.preventDefault();
    e.stopPropagation();
    // Text first, then the bar.
    if (input.value || bar.lock || bar.pin) {
      clearField();
      announce('Cleared');
    } else {
      window.aeval.invoke('overlay:collapse');
    }
    return;
  }
  if (e.key !== 'Enter') return;
  // Shift+Enter starts a new line.
  if (e.shiftKey && !e.ctrlKey && !e.altKey) return;
  e.preventDefault();
  onEnter(e);
});

// Up recalls older entries while the field is empty or still showing one;
// Down walks back toward what was being typed.
function recallKey(dir) {
  const h = bar.history;
  if (!h.length) return false;
  if (bar.recall < 0 && (dir > 0 || input.value)) return false;
  if (bar.recall < 0) {
    bar.draft = input.value;
    bar.recall = h.length;
  }
  const next = bar.recall + dir;
  if (next >= h.length) {
    bar.recall = -1;
    input.value = bar.draft;
  } else if (next >= 0) {
    bar.recall = next;
    input.value = h[next];
  } else {
    return true;
  }
  setLock(null);
  takePrefix();
  classifyNow();
  input.setSelectionRange(input.value.length, input.value.length);
  renderExamplesShown();
  return true;
}

// The bar's keyboard layer (U19). Pages see keys before the bridge blocks
// WebView2's own (Ctrl+R reload, Ctrl+W close, Ctrl+H history, Ctrl+N new
// window), so these chords are free to mean what they mean here. Chords on
// the target window wait for Rust to name one (overlay:state.target).
function invokeSoft(channel, payload) {
  return window.aeval.invoke(channel, payload).catch(() => null);
}

function targetLabel() {
  return state.target && state.target.label ? String(state.target.label) : '';
}

function newConversation() {
  invokeSoft('chat:new');
  announce('The next question starts a new conversation.');
}

function closeTarget(all) {
  if (!targetLabel()) return;
  if (all) invokeSoft('output:close-all');
  else invokeSoft('output:close', { label: targetLabel() });
}

function copyLastAnswer() {
  const label = targetLabel();
  if (!label) return;
  invokeSoft('output:last-answer', { label }).then((r) => {
    const text = typeof r === 'string' ? r : r && typeof r.text === 'string' ? r.text : '';
    if (!text) return;
    const done = () => announce('Copied the last answer.');
    if (navigator.clipboard) navigator.clipboard.writeText(text).then(done, () => {});
  });
}

function stopRunning() {
  if (state.agent && state.agent.running) invokeSoft('agent:stop');
  else invokeSoft('query:cancel');
}

const BAR_CHORDS = {
  'Ctrl+n': newConversation,
  'Ctrl+w': () => closeTarget(false),
  'Ctrl+Shift+w': () => closeTarget(true),
  'Ctrl+Shift+c': copyLastAnswer,
  'Ctrl+.': stopRunning,
  'Ctrl+Shift+r': () => invokeSoft('query:retry', targetLabel() ? { label: targetLabel() } : {}),
  'Ctrl+r': () => invokeSoft('region:pick'),
  'Ctrl+,': () => invokeSoft('overlay:open-settings'),
  'Ctrl+h': () => invokeSoft('overlay:open-history'),
  'Ctrl+m': () => invokeSoft('overlay:minimize'),
};

function chordOf(e) {
  let k = e.key.length === 1 ? e.key.toLowerCase() : e.key;
  // Shift turns . and , into > and < on most layouts; the code is steadier.
  if (e.code === 'Period') k = '.';
  else if (e.code === 'Comma') k = ',';
  return `${e.ctrlKey ? 'Ctrl+' : ''}${e.altKey ? 'Alt+' : ''}${e.shiftKey ? 'Shift+' : ''}${k}`;
}

document.addEventListener('keydown', (e) => {
  if (state.mode !== 'panel' || e.defaultPrevented || e.isComposing || e.metaKey) return;
  const run = BAR_CHORDS[chordOf(e)];
  if (!run) return;
  e.preventDefault();
  run();
});

// Escape anywhere else in the window: the menu, then the bar.
window.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && !e.defaultPrevented) {
    if (menuOpen()) closeMenu(true);
    else window.aeval.invoke('overlay:collapse');
  }
  if (e.key === 'F6' && !e.defaultPrevented) window.aeval.invoke('ui:focus-answer');
});

// ------------------------------------------------------------- announcer

// One voice for the bar: notices, status notes and the guess all speak
// through AevalA11y (glass.js), which uses this window's live regions while
// it has focus and asks Rust to say it elsewhere when it does not. Errors
// and a task's question are assertive; everything else is polite.
function announce(text, priority) {
  const t = String(text || '');
  if (window.AevalA11y) {
    window.AevalA11y.say(t, priority === 'assertive' ? 'assertive' : 'polite');
    return;
  }
  announcer.textContent = '';
  requestAnimationFrame(() => {
    announcer.textContent = t;
  });
}

let announceSoonTimer = null;
function announceSoon(text) {
  clearTimeout(announceSoonTimer);
  announceSoonTimer = setTimeout(() => announce(text), 500);
}

// ------------------------------------------------------------- from main

// The bar opens unlocked, guessing, unless a flow asked for one action.
window.aeval.on('bar:open', ({ lock }) => {
  setLock(lock === 'agent' ? 'do' : lock || null);
});

window.aeval.on('bar:prefill', ({ text, mode }) => {
  input.value = text || '';
  setLock(mode === 'agent' ? 'do' : mode === 'ask' ? null : bar.lock);
  input.focus();
  renderExamplesShown();
});

// A line from Rust run as if typed here and Entered (windows::submit_text):
// the same prefixes, the same guess, the same Enter.
window.aeval.on('bar:submit', ({ text }) => {
  input.value = String(text || '');
  bar.recall = -1;
  setLock(null);
  takePrefix();
  classifyNow();
  renderExamplesShown();
  onEnter({ altKey: false });
});

// A task that started from the bar leaves it; one waiting on consent keeps
// its words until it really starts.
window.aeval.on('agent:begin', ({ task }) => {
  if (input.value.trim() === String(task || '').trim()) clearField();
});

function barOpened() {
  bar.recall = -1;
  // A key added since the last time shows up in the @ list.
  bar.models = null;
  window.aeval.invoke('bar:history').then((h) => {
    if (Array.isArray(h)) bar.history = h;
  });
  // A menu or the key hints left open when the bar closed (the minimize
  // button, the shortcut) do not come back with it: the window opens at the
  // bar's own height, so they would be cut off until it grew.
  for (const m of [intentMenu, modelMenu]) {
    window.AevalGlass.menuIn(m);
    m.hidden = true;
  }
  token.setAttribute('aria-expanded', 'false');
  input.removeAttribute('aria-activedescendant');
  input.removeAttribute('aria-controls');
  input.removeAttribute('aria-autocomplete');
  hideTip();
  renderToken();
  renderExamplesShown();
}

renderToken();
