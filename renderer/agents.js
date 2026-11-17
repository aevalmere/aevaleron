'use strict';

// The agents notification (src-tauri/src/agents/mod.rs), behaving as
// Aevalsistant's does (its src/Toast.cs). Rust sends `agents:card` with
// everything to draw; the page draws it out of sight, tells Rust how tall
// the window must be with the card open (`agents:size`, which places and
// shows the window at the top of the screen), and only then drops it in.
// `agents:hide` lifts it away, and the page says `agents:hidden` once the
// frame without it is on screen.
//
// The window lets the mouse through everywhere but the card. While it does,
// Rust forwards the pointer's moves here, so the page says (`agents:pass`)
// whether the pointer is on the card, and keeps hover from the same moves.
(() => {
  const card = document.getElementById('card');
  const head = document.getElementById('head');
  const text = document.getElementById('text');
  const title = document.getElementById('title');
  const place = document.getElementById('place');
  const d1 = document.getElementById('d1');
  const dn = document.getElementById('dn');
  const keys = document.getElementById('keys');
  const list = document.getElementById('list');

  // The card's place in the window, and the room under it for the shadow.
  const TOP = 16;
  const BOTTOM = 22;
  const HEAD = 64;
  const LINE = 16;
  // A pointer crossing the card on its way somewhere else does not open it.
  const OPEN_AFTER_MS = 150;
  // The longest exit (agents.css, lift).
  const LIFT_MS = 220;
  // A host name gives way to the project's down to this, then both shrink.
  const STUB = 64;

  let data = null;
  let shown = '';
  let grow = 0;
  let openTimer = null;
  let hideSeq = 0;
  let through = true;
  let hovering = false;
  let hot = null;
  let pointer = null;

  const send = (channel, payload) => {
    try {
      return Promise.resolve(window.aeval.invoke(channel, payload)).catch(() => null);
    } catch (err) {
      return Promise.resolve(null);
    }
  };

  function el(tag, cls, words) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (words !== undefined) e.textContent = words;
    return e;
  }

  // The frame after next is on screen, or 80 ms where no frames come.
  function painted(fn) {
    let done = false;
    const go = () => {
      if (done) return;
      done = true;
      fn();
    };
    requestAnimationFrame(() => requestAnimationFrame(go));
    setTimeout(go, 80);
  }

  // The host gives way first, but keeps a stub of itself.
  function stub(host) {
    host.style.minWidth = '';
    host.style.minWidth = `${Math.min(STUB, host.scrollWidth)}px`;
  }

  function row(r, i, next) {
    const child = r.depth > 0;
    const parent = !child && next && next.depth > 0;
    const last = child && !(next && next.depth > 0);
    const li = el('li', `row ${r.state}${child ? ' child' : ''}${parent ? ' parent' : ''}${last ? ' last' : ''}`);
    li.dataset.i = String(i);
    li.appendChild(el('span', 'dot'));
    const what = el('span', 'what');
    what.appendChild(el('span', 'name', r.name));
    if (r.place) what.appendChild(el('span', 'host', r.place));
    what.appendChild(el('span', 'status', r.text));
    li.appendChild(what);
    return li;
  }

  function draw(p) {
    data = p;
    card.dataset.kind = p.kind;
    title.textContent = p.title;
    place.textContent = p.place || '';
    place.hidden = !p.place;
    const detail = p.detail || '';
    d1.textContent = detail;
    dn.textContent = detail;
    card.classList.toggle('one-line', !detail);
    keys.hidden = !p.altTab;

    const rows = p.rows || [];
    list.textContent = '';
    rows.forEach((r, i) => list.appendChild(row(r, i, rows[i + 1])));
    if (p.more > 0) {
      const more = el('li', 'row more');
      more.appendChild(el('span', '', `${p.more} more in the tray menu`));
      list.appendChild(more);
    }
    list.hidden = !list.childElementCount;
    // The head's own children hang from under the avatar.
    card.classList.toggle('stem', rows.length > 0 && rows[0].depth > 0);

    text.style.right = p.altTab ? `${14 + keys.offsetWidth + 12}px` : '18px';
    stub(place);
    for (const h of list.querySelectorAll('.host')) stub(h);

    // How much taller the card gets open: every line past the first.
    const lines = detail && p.expand ? Math.max(1, Math.round(dn.offsetHeight / LINE)) : 1;
    grow = (lines - 1) * LINE;
    if (!grow) close();
    else if (card.classList.contains('open')) card.style.setProperty('--grow', `${grow}px`);
  }

  // The card closed, wherever its opening has got to.
  const closedHeight = () => card.offsetHeight - head.offsetHeight + HEAD;

  // The window is always as tall as the card open, so opening only redraws.
  function size() {
    return send('agents:size', { height: TOP + closedHeight() + grow + BOTTOM });
  }

  function open() {
    if (!grow || card.hidden) return;
    card.classList.add('open');
    card.style.setProperty('--grow', `${grow}px`);
  }

  function close() {
    clearTimeout(openTimer);
    card.classList.remove('open');
    card.style.removeProperty('--grow');
  }

  // ------------------------------------------------------------- pointer

  function setThrough(v) {
    if (v === through) return;
    through = v;
    send('agents:pass', { ignore: v });
  }

  function onCard(x, y) {
    if (card.hidden || card.classList.contains('out')) return false;
    const r = card.getBoundingClientRect();
    return x >= r.left && x < r.right && y >= r.top && y < r.bottom;
  }

  function setHot(li) {
    if (li === hot) return;
    if (hot) hot.classList.remove('hot');
    hot = li;
    if (hot) hot.classList.add('hot');
  }

  function leave() {
    pointer = null;
    setThrough(true);
    setHot(null);
    if (!hovering) return;
    hovering = false;
    card.classList.remove('hover');
    send('agents:hover', { on: false });
    close();
  }

  function move(x, y) {
    pointer = { x, y };
    const on = onCard(x, y);
    setThrough(!on);
    if (!on) {
      leave();
      pointer = { x, y };
      return;
    }
    const at = document.elementFromPoint(x, y);
    const li = at && at.closest('#list .row[data-i]');
    setHot(li || null);
    if (hovering) return;
    // The pointer on the card holds the countdown, and once it settles
    // opens the whole message.
    hovering = true;
    card.classList.add('hover');
    send('agents:hover', { on: true });
    if (data && data.expand && grow) openTimer = setTimeout(open, OPEN_AFTER_MS);
  }

  // Real moves while the card has the mouse; moves Rust forwards while the
  // window lets it through.
  document.addEventListener('mousemove', (e) => move(e.clientX, e.clientY));
  document.addEventListener('mouseleave', leave);
  window.addEventListener('mouseout', (e) => {
    if (!e.relatedTarget) leave();
  });

  card.addEventListener('click', (e) => {
    if (!data || card.classList.contains('out')) return;
    const li = e.target.closest('#list .row[data-i]');
    if (li) {
      const r = (data.rows || [])[Number(li.dataset.i)];
      if (r) send('agents:open', { session: r.session });
      return;
    }
    // The head goes to its session's window; the list's margins and the
    // "more" line do nothing.
    if (e.target.closest('#head')) send('agents:open', {});
  });

  // A right click puts it away, and the ones under it.
  card.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    send('agents:dismiss');
  });

  // ------------------------------------------------------------- show, hide

  window.aeval.on('agents:card', (p) => {
    if (!p) return;
    const key = [p.session, p.kind, p.title, p.detail].join('\n');
    const leaving = card.hidden || card.classList.contains('out');
    hideSeq += 1;
    if (leaving) {
      card.classList.remove('out', 'in');
      card.classList.add('measuring');
      close();
    } else if (key !== shown) {
      close();
    }
    // Drawn where it can be measured.
    card.hidden = false;
    draw(p);
    shown = key;
    if (!leaving) {
      // Already up: the newer one takes its place without moving, as
      // Aevalsistant's does.
      size();
      return;
    }
    // From just above the top edge: the card and the room round it.
    card.style.setProperty('--from', `${-(closedHeight() + TOP + BOTTOM)}px`);
    size().then(() => {
      card.classList.remove('measuring');
      void card.offsetWidth; // restart the animation
      card.classList.add('in');
      if (!pointer) setThrough(true);
    });
  });

  // Landed under a pointer that has not moved: it is on the card now.
  card.addEventListener('animationend', (e) => {
    // `show` alone when Windows animations are off.
    const landed = e.animationName === 'drop' || e.animationName === 'show';
    if (landed && pointer && card.classList.contains('in')) move(pointer.x, pointer.y);
  });

  window.aeval.on('agents:hide', () => {
    close();
    card.classList.remove('in', 'hover');
    card.classList.add('out');
    setHot(null);
    hovering = false;
    shown = '';
    // The window hides when the lift is over and painted, so it never goes
    // with a card on it and never comes back showing the old one.
    const seq = ++hideSeq;
    const gone = () => {
      if (seq !== hideSeq || card.hidden || !card.classList.contains('out')) return;
      card.hidden = true;
      painted(() => {
        if (seq === hideSeq && card.hidden) send('agents:hidden');
      });
    };
    Promise.all(card.getAnimations().map((a) => a.finished)).then(gone, gone);
    // A page that is not being painted never finishes an animation.
    setTimeout(gone, LIFT_MS + 60);
  });

  // Last, so every listener above is live before Rust sends the card. Rust
  // made the window let the mouse through; say so too after a reload.
  send('agents:pass', { ignore: true });
  send('agents:ready');
})();
