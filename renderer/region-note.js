// The copy-text confirmation (src-tauri/src/region_text/note.rs): shows what
// Rust sends and fades it after its time. Rust hides the window once the
// fade is over.
(() => {
  const title = document.getElementById('title');
  const lead = document.getElementById('lead');
  let timer = null;

  window.aeval.on('region:note', (p) => {
    if (!p) return;
    clearTimeout(timer);
    title.textContent = p.title || '';
    lead.textContent = p.lead || '';
    document.body.dataset.kind = p.kind || 'idle';
    const ms = Number(p.ms) || 0;
    // The words stay while the capsule fades, so it does not empty first.
    if (ms > 0) {
      timer = setTimeout(() => {
        document.body.dataset.kind = 'idle';
      }, ms);
    }
  });

  // The listener is in place: Rust may send what it holds.
  window.aeval.invoke('region:note-ready').catch(() => {});
})();
