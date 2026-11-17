// What the person means by what they typed: Ask or Do.
//
// Pure and local: no Electron, no network, no model. The bar calls it on
// every keystroke, so it has to finish in well under 5 ms; the smoke run
// times it and checks it against a table of phrases.
//
// A clear order is a task and Enter starts it, through exactly the path a
// leading > takes, so the executor's safety gate (agent/gate.rs) decides
// what needs a yes; nothing here is read by the gate. A clear question is
// an answer. Anything in between is asked as a question, and the answer
// offers "Do this instead" (offerDo), because a wrong answer costs a second
// and a wrong task can click things.
//
// Loaded two ways: require() in the smoke run, and a plain <script> in the
// bar (window.AevalIntent), hence the wrapper.

(function (root) {
'use strict';

const INTENTS = ['ask', 'do'];
const NAME = { ask: 'Ask', do: 'Do' };

// Words that start a question, and asking where something is.
const ASK_START =
  /^(what|what's|whats|why|how|who|whom|whose|when|which|where|where's|wheres|is|are|was|were|does|did|can|could|should|would|will|has|have|explain|summari[sz]e|translate|tell me|meaning|define|describe|compare|any idea|i don'?t|i'?m|im|is there|are there|whats? up with|find|locate|look for|show me where)\b/i;
// "how do I save this" is a question about a verb, not an order to do it.
const HOW_TO = /^(how (do|can|should|would) (i|you|we)|how to|is it possible to|what'?s the (best )?way to|can i|should i)\b/i;
// The orders the agent can carry out. "copy" and "write" are left out on
// purpose: "copy this paragraph" and "write me a haiku" are mostly asks for
// text back, not for clicks.
const DO_VERBS =
  'open|launch|start|close|quit|click|double-click|right-click|press|type|rename|move|paste|save|send|reply|forward|delete|remove|erase|create|make a new|new folder|fill in|fill out|fill|download|upload|install|uninstall|scroll|select all|select|highlight|check|uncheck|tick|untick|enable|disable|turn on|turn off|switch to|switch|go to|jump to|navigate to|search for|search|log in|login|sign in|sign out|log out|attach|book|schedule|sort|drag|minimi[sz]e|maximi[sz]e|resize|set|change|add|insert|empty|archive|submit|post|print|run|play|pause|mute|unmute|zip|unzip|extract|convert|export|import|share|pin|unpin|bookmark|refresh|reload|maximise|restart|shut down|lock|order|buy|pay|transfer|accept|decline|approve|merge|deploy|format|reset|sign';
const DO_START = new RegExp(`^(please |pls |can you |could you |would you |go ahead and |now |then )?(${DO_VERBS})\\b`, 'i');
const DO_FOLLOW = /^(do it|do that|do this|go ahead|yes,? do (it|that)|carry (it|that) out|make it so)\b/i;
const POLITE = /^(can|could|would) you\b/i;
const FOR_ME = /\b(for me|on my computer|on my behalf)\b/i;
const CHAIN = /\b(and then|then)\b|->/i;
// A request that only needs words back.
const TEXT_ONLY = /^(write|draft|compose|rewrite|reword|proofread|translate|summari[sz]e|explain|list|give me|suggest|generate)\b/i;
// Verbs whose task is worth a second look. The bar treats them like any
// other task; the gate does the checking.
const RISKY =
  /\b(send|delete|remove|erase|pay|buy|order|purchase|submit|post|share|install|uninstall|format|reset|transfer|sign|reply|forward|merge|deploy|empty|wipe|publish)\b/i;
const VISUAL = /\b(which (button|icon|one|tab|option)|where (is|are|do)|colou?r|icon|button|look(s)? like|see|picture|image|photo|chart|graph|layout|design|logo|font|screenshot|highlighted|red|green|blue)\b/i;

// A task has to score at least DO_FLOOR, and beat the question by
// DO_MARGIN, before Enter starts it.
const DO_FLOOR = 0.6;
const DO_MARGIN = 0.15;
// Below that, a line with this much of a task in it is asked, and the
// answer offers to do it instead.
const OFFER_DO = 0.2;

function words(s) {
  return String(s || '').trim().split(/\s+/).filter(Boolean);
}

/**
 * Splits a typed line into its prefix lock and the text after it.
 *
 *   ?   Ask        >   Do
 *
 * @param {string} raw
 * @returns {{lock: 'ask'|'do'|null, prefix: string, body: string}}
 */
function parse(raw) {
  const s = String(raw || '');
  if (s.startsWith('?')) return { lock: 'ask', prefix: '?', body: s.slice(1).replace(/^\s+/, '') };
  if (s.startsWith('>')) return { lock: 'do', prefix: '>', body: s.replace(/^>+/, '').replace(/^\s+/, '') };
  return { lock: null, prefix: '', body: s };
}

/**
 * Scores what an unlocked line most likely means.
 *
 * @param {string} text the line without a prefix
 * @returns {{intent: 'ask'|'do', scores: object, margin: number, offerDo: boolean, why: string[], risky: boolean, visual: boolean}}
 */
function score(text) {
  const t = String(text || '').trim();
  const why = [];
  const s = { ask: 0.3, do: 0 };
  if (!t) return finish(s, why, t);
  const n = words(t).length;
  const asks = /\?\s*$/.test(t);
  const doStart = DO_START.test(t);
  const howTo = HOW_TO.test(t);

  if (asks) {
    s.ask += 0.6;
    why.push('ends with a question mark');
  }
  if (ASK_START.test(t) && !(POLITE.test(t) && doStart)) {
    s.ask += 0.5;
    why.push('starts like a question');
  }
  if (howTo) {
    s.ask += 0.3;
    why.push('asks how to do something');
  }

  const follow = DO_FOLLOW.test(t);
  if (doStart && !howTo) {
    // A lone verb ("save") is ambiguous; an order has an object.
    s.do += n >= 2 ? 0.65 : 0.2;
    why.push(n >= 2 ? 'starts with something to do' : 'a single verb');
    if (asks) s.do -= 0.3;
  }
  if (follow) {
    s.do += 0.75;
    why.push('asks to carry something out');
  }
  if (s.do > 0 && FOR_ME.test(t)) s.do += 0.15;
  if (s.do > 0 && CHAIN.test(t)) s.do += 0.1;
  if (TEXT_ONLY.test(t) && !doStart) {
    s.ask += 0.3;
    why.push('wants words back');
  }
  if (n >= 6 && !doStart && !follow) {
    s.ask += 0.15;
    why.push('a long sentence');
  }
  return finish(s, why, t);
}

function finish(s, why, t) {
  const ask = Math.round(s.ask * 100) / 100;
  const run = Math.round(s.do * 100) / 100;
  const clearDo = run >= DO_FLOOR && run - ask >= DO_MARGIN;
  return {
    intent: clearDo ? 'do' : 'ask',
    scores: { ask, do: run },
    margin: Math.round(Math.abs(ask - run) * 100) / 100,
    offerDo: !clearDo && run >= OFFER_DO,
    why,
    risky: RISKY.test(t),
    visual: VISUAL.test(t),
  };
}

/**
 * parse + score: the whole decision for one typed line.
 *
 * @param {string} raw what is in the field, prefix included
 */
function classify(raw) {
  const p = parse(raw);
  const sc = score(p.body);
  if (p.lock) return { ...sc, intent: p.lock, offerDo: false, locked: true, prefix: p.prefix, body: p.body };
  return { ...sc, locked: false, prefix: '', body: p.body };
}

// Starting points under an empty bar, picked from the app in front: a
// question and a task for each. Each one fills the field, so the token also
// teaches what Enter will do with it.
const EXAMPLES = {
  browser: ['Summarize this page', 'Where is the sign in button?', 'Open a new tab and search for the weather'],
  excel: ['What is the total of this column?', 'Explain this formula', 'Sort this sheet by date'],
  mail: ['Draft a reply to this email', 'What does this email want from me?', 'Archive this email'],
  editor: ['What does this error mean?', 'Explain this function', 'Save all open files'],
  terminal: ['Why did this command fail?', 'What does this output mean?', 'Open a new terminal tab'],
  explorer: ['Which of these files is the newest?', 'Rename these files by date', 'Make a new folder called old'],
  document: ['Summarize this document', 'Fix the grammar in this paragraph', 'Save this as a PDF'],
  other: ["What's on my screen?", 'Where is the settings button?', 'Open Notepad and write a haiku'],
};

function appKind(processName, title) {
  const p = String(processName || '').toLowerCase().replace(/\.exe$/, '');
  const t = String(title || '').toLowerCase();
  if (/^(chrome|msedge|firefox|brave|opera|vivaldi|arc)$/.test(p)) return 'browser';
  if (p === 'excel') return 'excel';
  if (/^(outlook|olk|thunderbird|hxoutlook)$/.test(p) || /\b(inbox|mail)\b/.test(t)) return 'mail';
  if (/^(code|devenv|idea64|pycharm64|sublime_text|notepad\+\+|cursor|rider64|webstorm64)$/.test(p)) return 'editor';
  if (/^(windowsterminal|cmd|powershell|pwsh|conhost|wt|mintty|alacritty)$/.test(p)) return 'terminal';
  if (p === 'explorer') return 'explorer';
  if (/^(winword|notepad|wordpad|acrord32|acrobat|sumatrapdf|onenote|obsidian|notion)$/.test(p)) return 'document';
  return 'other';
}

function examples(processName, title) {
  return EXAMPLES[appKind(processName, title)].slice();
}

const api = { INTENTS, NAME, parse, score, classify, examples, appKind, RISKY, VISUAL };
if (typeof module !== 'undefined' && module.exports) module.exports = api;
else root.AevalIntent = api;
})(typeof window !== 'undefined' ? window : globalThis);
