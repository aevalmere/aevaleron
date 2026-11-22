# Lane: cli-providers

Branch `rust/cli-providers`, cut from `origin/rust/next`. The app can send every request through an installed AI command-line tool on the person's own subscription (Claude Code, OpenAI Codex, Gemini CLI) instead of an HTTP API with a key. HTTP providers are untouched and stay the default: a CLI is used only once it is turned on.

Terms: for the person's own use this is the CLIs' intended headless mode. Shipping an app that routes *other people's* subscriptions through these CLIs may conflict with the providers' terms (consumer plans are generally licensed for the subscriber's own interactive and scripted use, not as a backend for a third-party product). Check each provider's current terms before this is offered to users at large.

## How it fits

- Three new `Provider` variants: `ClaudeCli` (`claude-cli`), `CodexCli` (`codex-cli`), `GeminiCli` (`gemini-cli`). `Provider::CLI` lists them; `Provider::ALL` is still the four HTTP providers. `is_cli()` is new; `is_native()` is true for them (own model list, no free-model failover).
- A CLI has no key. Its "key" (`Active.key`, `Candidate.key`, the status table's key) is the path of its program, there only when the CLI is turned on and found. So candidates, failover across providers, the status table (not signed in = rejected, plan limit = busy) and recipients all work unchanged.
- `Llm::request` sends a CLI provider's request to `llm::cli::send`, which runs the CLI and returns an OpenRouter-dialect `Reply` (SSE for streams, one JSON body otherwise). Everything above it (`stream_query`, `read_screen`, `stream_text`, `chat_turn`, the agent's reviewer, voice cleanup) needed no change.
- `supports_tools` is false for a CLI: the agent drives it with the one-JSON-object fallback it already uses for OpenAI.

## API for the UI lane

Rust (call directly from the Slint side, or through `host::settings::route` with the channel names below; they are not in `ipc::INVOKE_CHANNELS`, so no web page can call them):

| Call | Channel | Payload | Returns |
|---|---|---|---|
| `llm::cli::detect().await -> Vec<CliInfo>` (wrapped by `host::settings::cli_detect().await`) | `settings:cli-detect` | none | `{ok, clis: [CliRow]}` |
| `host::settings::set_cli(app, &p).await` | `settings:set-cli` | `{provider: "claude-cli"\|"codex-cli"\|"gemini-cli", enabled?: bool, path?: string\|null, makeDefault?: bool}` | `{ok, cli: CliRow}` |
| `host::settings::set_role(app, &p)` | `settings:set-role` | `{role: "vision"\|"text"\|"agent"\|"voice", provider: id\|null}` | `{ok, roleProviders}` |
| existing `settings:test-key {provider: "claude-cli"}` | | | `{ok, status, message}`: runs `--version` and the sign-in status command, never a prompt |
| existing `settings:set-provider-order {order}` | | now accepts the CLI ids | |
| existing `settings:save-model {provider: "claude-cli", model}` (and `save-brain`, `save-agent-model`) | | stores `nativeModels.claude-cli.<kind>` | |
| existing `models:list {provider: "claude-cli", kind}` | | the curated list, label "Claude Code (your plan)" | |

`CliInfo` (serde camelCase): `id`, `name` ("Claude Code", "Codex", "Gemini CLI"), `command` ("claude", "codex", "gemini"), `installed`, `path`, `version` ("2.1.330"), `signedIn` (`true`/`false`/`null` when the CLI can't say). `CliRow` is `CliInfo` plus `enabled` and `customPath`.

`settings:get` gains `clis: [CliRow]` (from the last detect, else PATH only, no version) and `roleProviders: {vision, text, agent, voice}` (provider id or `null`). Its `providers` rows include each turned-on CLI: `hasKey: true`, `keyHint` = the program's file name, status from the table.

Error codes the UI may see in `query:error`: the usual `bad-key` (here: not signed in, text "Claude Code isn't signed in. Open a terminal, run `claude` and sign in, then ask again."), `rate-limited`/`daily-cap` (plan usage limit, "See usage" action), and one new code `cli-missing` (program gone or would not start; action `fix-cli` opens section `prov-<id>`).

Store (aevaleron.json, raw keys):

```json
"cliProviders": { "enabled": ["claude-cli"], "paths": { "codex-cli": "C:\\path\\codex.cmd" } },
"roleProviders": { "vision": "claude-cli", "voice": "claude-cli" },
"providerOrder": ["claude-cli", "openrouter", ...],
"nativeModels": { "claude-cli": { "vision": "sonnet", "text": "opus" } }
```

- Turned on but never placed in `providerOrder`: tried after the HTTP providers.
- As the default: `set-cli {makeDefault: true}` (or `set-provider-order`) puts it first.
- Per job: `roleProviders.<role>` is tried first for that job (`candidates_for(kind)` reads `vision`/`text`/`agent`). `voice` is read by the voice cleanup only; a CLI is used for cleanup only when picked there, with its fastest model (`haiku` for Claude) and an 8 s budget instead of 800 ms.

## Detection

`llm::cli::find(p)` (cached 30 s): `<cmd>.exe` then `<cmd>.cmd` in this process's PATH, the HKCU and HKLM `Path` from the registry (expanded), `%APPDATA%\npm`, `%USERPROFILE%\.local\bin`, `%USERPROFILE%\.bun\bin`. An npm `.cmd` shim is never run through `cmd.exe` (its quoting can't carry a multi-line prompt): the `.js` it names is run with the `node.exe` beside it or on PATH. A `.cmd` that is not an npm shim is refused.

## Flags per CLI

Checked against the help of the versions installed on the dev PC on 2026-11-21. No real prompt was run through any CLI.

**Claude Code 2.1.330** (`claude.exe`, native installer):
`-p --verbose --input-format stream-json --output-format stream-json --include-partial-messages --no-session-persistence --safe-mode --strict-mcp-config --disable-slash-commands --permission-mode dontAsk --tools "" [--model=<m>] [--effort low] [--system-prompt=<s>]`. The message goes in on stdin as one stream-json `user` line; the screenshot is a base64 `image` block in it, so no file is written and no Read tool is needed (stronger than the planned "Read only": no tools at all). Env `DISABLE_AUTOUPDATER=1`. Output: `stream_event` text deltas, then `result` (usage, `is_error`). Models: `opus`, `sonnet` (screen default), `haiku`, or any full name. A system prompt over 16 000 characters goes into the message instead.

**Codex CLI 0.166.0** (npm shim `codex.cmd` -> `node codex.js`):
`exec --json --color never --ephemeral --skip-git-repo-check --ignore-user-config --ignore-rules --sandbox read-only --cd <dir> --disable shell_tool --disable unified_exec --disable apps --disable plugins --disable multi_agent --disable image_generation --disable computer_use --disable browser_use --disable hooks --disable view_image [--model <m>] [--config model_reasoning_effort=low] --image=<file>...`. Prompt on stdin (no positional), the system prompt prepended in `<instructions>`. Feature names from `codex features list` on 0.166.0. Output: `item.*` `agent_message` text (grows per update), `turn.completed` usage, `turn.failed`. Model `default` sends no `--model`.

**Gemini CLI**: not installed on the dev PC; flags are from its documentation and NOT checked against a binary: `--output-format stream-json --approval-mode default --extensions none [--model <m>] --prompt=<text>` (text over 16 000 characters goes on stdin and `--prompt` says "Answer the message above."). System prompt through `GEMINI_SYSTEM_MD=<file>`. Images as `@<file>` in the text, the file in the working folder. Output: `message` (assistant, `delta: true`), `result`. Exit 41 = not signed in.

## Process handling

- `CREATE_NO_WINDOW`; stdin/stdout/stderr piped; `kill_on_drop`.
- Each run gets its own job object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`; dropping the run (answer done, cancel, the caller's timeout, a 10-minute hard cap) terminates the job, so the CLI and everything it started (Codex's node script starts a native binary) die, and the system does the same if the app dies.
- Working folder: `%TEMP%\aevaleron-cli\<id>`, one per CLI so the CLIs that record a project per folder see one project. Image and system-prompt files are named per run and deleted when it ends (retried off-thread while a dying process holds them; anything older than an hour is swept).
- Latency: the process is spawned as soon as `send` is called and its stdin (with the image, for Claude) is written while it boots. Streams return on the first text delta. Nothing is kept or reused between requests.
- Failures: the CLI's `result`/`turn.failed`/stderr text and exit code are mapped: not signed in -> 401 (`bad-key`), plan limit -> 429, unknown model -> 404, connection errors -> offline, anything else -> 500 with the CLI's first line; a program that is gone or won't start -> `cli-missing`.

## Files touched outside `src-tauri/src/llm/`

- `src-tauri/src/host/settings.rs`: `LlmSettings` for the new store keys, CLI rows, `settings:cli-detect`, `settings:set-cli`, `settings:set-role`, CLI test, CLI "not signed in" status text.
- `src-tauri/src/voice/cleanup.rs`: a CLI picked for voice is used, with its fast model and budget.
- This note.

## Verified

- CI (rust.yml) on the branch: fmt, clippy, tests. Unit tests with a fake CLI (`src/llm/cli/fake_cli.js`, run with node, playing each of the three CLIs by its command line): streaming parse for each CLI, whole replies for the agent/reader, the screenshot reaching each CLI (stdin block for Claude, file for Codex and Gemini, deleted after), Gemini's system prompt file, cancel killing the CLI and its child process, a silent CLI timing out as 408 and being killed, not signed in and usage limit mapped to the app's errors and copy, a missing program, version and sign-in detection. Parsers are also tested on recorded-shape lines.

## Not verified

- No real prompt through any CLI (it would spend the subscription). The stream-json image block for Claude, `--safe-mode` together with `-p`, Codex's `--disable` names, and the `model_reasoning_effort` key are from help/docs only.
- Gemini CLI entirely (not installed).
- `claude auth status --json` field name (`loggedIn`) and `codex login status` wording are assumptions; unknown output reads as "can't say" (`signedIn: null`), never as signed out.
- Prewarming a CLI before the question is known (spawning while the screenshot is still being taken) is not done: the request body, model and system prompt are only known at `send`.
- The UI: nothing in `renderer/` or `settings/` shows the CLIs yet (UI lane).
