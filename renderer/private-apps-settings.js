// Settings' Private apps section (settings.html #sec-private). Loads after
// settings.js and uses its helpers: call, failed, $, el, makeStatus,
// unsaved, setSummary. Every change is saved at once; privacy:add and
// privacy:remove answer with the whole section's state, so the list shown
// is always the list stored.
(() => {
  if (!document.getElementById('sec-private')) return;
  let snap = null;
  // Changes sent so far, and those not yet answered: a reload that started
  // before a change, or lands during one, is out of date and is dropped.
  let changes = 0;
  let pending = 0;

  const p = {
    list: $('private-list'),
    empty: $('private-empty'),
    name: $('private-name'),
    add: $('private-add'),
    front: $('private-add-front'),
    frontNote: $('private-front-note'),
    statusLine: $('private-status'),
  };
  const say = makeStatus(p.statusLine);

  // An error stays until the next change; anything else clears by itself.
  function status(text, error) {
    p.statusLine.classList.toggle('error', !!error);
    say(text, !!error);
  }

  function render() {
    if (!snap) return;
    const apps = snap.apps;
    // A reload redraws the rows; the Remove button that had the keyboard
    // keeps it.
    const active = document.activeElement;
    const held = active && p.list.contains(active) ? active.dataset.exe : null;
    p.list.textContent = '';
    for (const a of apps) {
      const row = el('li', { class: 'private-row' });
      const text = el('span', { class: 'private-app' }, a.name);
      text.appendChild(el('span', { class: 'private-exe' }, a.exe));
      const remove = el('button', { type: 'button', class: 'g-btn quiet-danger', 'aria-label': `Remove ${a.name}` }, 'Remove');
      remove.dataset.exe = a.exe;
      remove.addEventListener('click', () => removeApp(a));
      row.append(text, remove);
      p.list.appendChild(row);
    }
    if (held) {
      const again = [...p.list.querySelectorAll('button')].find((b) => b.dataset.exe === held);
      if (again) again.focus();
    }
    p.list.hidden = !apps.length;
    p.empty.hidden = !!apps.length;
    setSummary('private-summary', !apps.length ? 'None' : apps.length === 1 ? '1 app' : `${apps.length} apps`);

    // The app that was in front when the bar last opened, named beside the
    // button that adds it.
    const front = snap.front;
    p.front.disabled = !front || front.listed;
    if (!front) p.frontNote.textContent = 'Open the bar over an app, then come back here.';
    else if (front.listed) p.frontNote.textContent = `${front.name} is already on the list.`;
    else p.frontNote.textContent = `${front.name} (${front.exe})`;
  }

  function take(r) {
    if (failed(r) || !r || !Array.isArray(r.apps)) return false;
    snap = r;
    render();
    return true;
  }

  function load() {
    const at = changes;
    return call('privacy:get').then((r) => {
      if (at === changes && !pending) take(r);
    });
  }

  function change(channel, payload) {
    changes += 1;
    pending += 1;
    return call(channel, payload, 8000).then((r) => {
      pending -= 1;
      return r;
    });
  }

  function addApp(payload, back) {
    return change('privacy:add', payload).then((r) => {
      if (!take(r)) {
        unsaved(status);
        return;
      }
      if (r.error) {
        status(r.error, true);
        back.focus();
        return;
      }
      const a = r.added || {};
      status(a.new ? `${a.name} is on the list. Aevaleron will not read it.` : `${a.name} is already on the list.`);
      if (back === p.name) p.name.value = '';
      // The front button is off once its app is listed; the field takes over.
      (back.disabled ? p.name : back).focus();
    });
  }

  function removeApp(a) {
    const buttons = [...p.list.querySelectorAll('button')];
    const at = buttons.findIndex((b) => b.dataset.exe === a.exe);
    change('privacy:remove', { exe: a.exe }).then((r) => {
      if (!take(r)) {
        unsaved(status);
        return;
      }
      status(`${a.name} was taken off the list.`);
      // The row is gone: the next one's button takes the keyboard, else the
      // one before, else the name field.
      const left = [...p.list.querySelectorAll('button')];
      (left[Math.min(at, left.length - 1)] || p.name).focus();
    });
  }

  function addTyped() {
    const name = p.name.value.trim();
    if (!name) {
      status("Type the name of the app's program first, for example notepad.exe.", true);
      p.name.focus();
      return;
    }
    addApp({ name }, p.name);
  }

  p.add.addEventListener('click', addTyped);
  p.name.addEventListener('keydown', (e) => {
    if (e.key !== 'Enter') return;
    e.preventDefault();
    addTyped();
  });
  p.front.addEventListener('click', () => addApp({ front: true }, p.front));

  // The app in front changes while Settings is in the background.
  window.addEventListener('focus', load);
  load();
})();
