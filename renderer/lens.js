'use strict';

// Draws what lib/lens.js sends: the agent's control frame and its chip, on
// the display the agent is using.

const frameEl = document.getElementById('frame');
const chip = document.getElementById('chip');

window.aeval.on('lens:draw', ({ frame }) => {
  frameEl.classList.toggle('on', !!frame);
  chip.classList.toggle('on', !!(frame && frame.chip));
  chip.textContent = frame && frame.chip ? frame.chip : '';
});

// Last, so the listener above is live before main sends the first draw.
window.aeval.invoke('lens:ready');
