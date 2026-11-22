// A stand-in for claude, codex and gemini in llm::cli's tests. It never
// talks to a model. The command line says which CLI it plays (`exec` is
// Codex, `--input-format` is Claude Code, anything else Gemini CLI) and the
// model name what it does: ok, echo, login, limit, hang, slow.
'use strict';
const fs = require('fs');
const path = require('path');
const { spawn } = require('child_process');

const argv = process.argv.slice(2);
const flavour = argv.includes('exec')
  ? 'codex'
  : argv.includes('--input-format')
    ? 'claude'
    : 'gemini';

function modelArg() {
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a.startsWith('--model=')) return a.slice('--model='.length);
    if (a === '-m' || a === '--model') return argv[i + 1];
  }
  return 'ok';
}

const out = (o) => process.stdout.write(JSON.stringify(o) + '\n');

function delta(text, sofar) {
  if (flavour === 'claude') {
    out({ type: 'stream_event', event: { type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text } } });
  } else if (flavour === 'codex') {
    out({ type: 'item.updated', item: { id: 'item_1', type: 'agent_message', text: sofar } });
  } else {
    out({ type: 'message', role: 'assistant', content: text, delta: true });
  }
}

function answer(chunks) {
  const whole = chunks.join('');
  if (flavour === 'claude') out({ type: 'system', subtype: 'init', tools: [] });
  if (flavour === 'codex') out({ type: 'thread.started', thread_id: 't' });
  if (flavour === 'gemini') out({ type: 'init', session_id: 's' });
  let sofar = '';
  for (const c of chunks) {
    sofar += c;
    delta(c, sofar);
  }
  if (flavour === 'claude') {
    out({ type: 'assistant', message: { content: [{ type: 'text', text: whole }] } });
    out({ type: 'result', subtype: 'success', is_error: false, result: whole, usage: { input_tokens: 5, output_tokens: 2 } });
  } else if (flavour === 'codex') {
    out({ type: 'item.completed', item: { id: 'item_1', type: 'agent_message', text: whole } });
    out({ type: 'turn.completed', usage: { input_tokens: 5, cached_input_tokens: 0, output_tokens: 2 } });
  } else {
    out({ type: 'result', status: 'success', stats: { input_tokens: 5, output_tokens: 2 } });
  }
}

function report(input) {
  const cwd = process.cwd();
  const images = [];
  for (const a of argv) {
    if (a.startsWith('--image=')) images.push(a.slice('--image='.length));
  }
  const text = input + '\n' + argv.join('\n');
  for (const m of text.matchAll(/@(\S+\.(?:jpg|png))/g)) images.push(path.join(cwd, m[1]));
  const seen = images.map((p) => ({
    path: p,
    exists: fs.existsSync(p),
    data: fs.existsSync(p) ? fs.readFileSync(p).toString('base64') : null,
  }));
  const md = process.env.GEMINI_SYSTEM_MD;
  return {
    flavour,
    argv,
    input,
    cwd,
    images: seen,
    systemMd: md && fs.existsSync(md) ? fs.readFileSync(md, 'utf8') : null,
  };
}

function fail(stdoutLines, stderr, code) {
  for (const l of stdoutLines) out(l);
  if (stderr) process.stderr.write(stderr + '\n');
  process.exitCode = code;
}

// A process that keeps writing the time to `beat`, and a child that
// writes to `beat.child`: both must die when the run is stopped.
// What the message says, with Claude Code's stream-json lines unwrapped.
function plain(input) {
  if (flavour !== 'claude') return input;
  const texts = [];
  for (const line of input.split('\n')) {
    try {
      for (const b of JSON.parse(line).message.content) if (b.type === 'text') texts.push(b.text);
    } catch (e) {
      // not a message line
    }
  }
  return texts.join('\n');
}

function hang(input, speak) {
  const m = /beat:(\S+)/.exec(plain(input) + '\n' + argv.join('\n'));
  const beat = m ? m[1] : path.join(process.cwd(), 'beat');
  const child = beat + '.child';
  const script = `setInterval(() => require('fs').writeFileSync(${JSON.stringify(child)}, String(Date.now())), 40)`;
  spawn(process.execPath, ['-e', script], { stdio: 'ignore' });
  setInterval(() => fs.writeFileSync(beat, String(Date.now())), 40);
  const ready = setInterval(() => {
    if (!fs.existsSync(child) || !fs.existsSync(beat)) return;
    clearInterval(ready);
    if (speak) delta('Thinking', 'Thinking');
  }, 20);
}

function main(input) {
  const mode = modelArg();
  if (mode === 'echo') return answer([JSON.stringify(report(input))]);
  if (mode === 'hang') return hang(input, true);
  if (mode === 'slow') return hang(input, false);
  if (mode === 'login') {
    if (flavour === 'claude') {
      const said = 'Invalid API key · Please run /login';
      return fail([
        { type: 'assistant', message: { content: [{ type: 'text', text: said }] }, error: 'authentication_failed' },
        { type: 'result', subtype: 'success', is_error: true, result: said },
      ], '', 1);
    }
    if (flavour === 'codex') return fail([], 'Error: Not logged in. Run `codex login` to sign in.', 1);
    return fail([], 'Please set an Auth method in your settings.json', 41);
  }
  if (mode === 'limit') {
    if (flavour === 'claude') {
      return fail([{ type: 'result', subtype: 'success', is_error: true, result: 'Claude AI usage limit reached|1767225600' }], '', 1);
    }
    if (flavour === 'codex') {
      return fail([{ type: 'turn.failed', error: { message: "You've hit your usage limit. Try again later." } }], '', 1);
    }
    return fail([{ type: 'result', status: 'error', error: { type: 'Error', message: 'You have exhausted your quota. It resets in 2h.' } }], '', 1);
  }
  return answer(['Hel', 'lo']);
}

if (argv[0] === '--version') {
  process.stdout.write('9.8.7 (Fake CLI)\n');
} else if (argv[0] === 'auth' && argv[1] === 'status') {
  process.stdout.write(JSON.stringify({ loggedIn: true, authMethod: 'claude.ai' }) + '\n');
} else if (argv[0] === 'login' && argv[1] === 'status') {
  process.stderr.write('Logged in using ChatGPT\n');
} else {
  let input = '';
  process.stdin.setEncoding('utf8');
  process.stdin.on('data', (d) => {
    input += d;
  });
  process.stdin.on('end', () => main(input));
}
