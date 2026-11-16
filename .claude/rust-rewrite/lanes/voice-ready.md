# Lane brief: voice-ready

Branch `rust/voice-ready`, cut from `rust-rewrite` at `ec8f2aa4`.

## Why

The owner installed `ec8f2aa4` on 2026-11-15 and reported the first dictation took 34 s ("way too slow"). Settings showed "text in 34 s after you let go".

What the PC showed: the default engine had changed from Whisper base to Parakeet 110M with that build, the Parakeet file was written at 12:55:52, and the app's process had used 55 s of processor time in half an hour. Thirty-four seconds of transcribing on eight threads would have used several times that. So the 34 s was the 101 MB model arriving inside the first dictation, with last week's Whisper model sitting on disk unused. This is the likely cause, not a measured one: the app kept no record of where a dictation's time went.

The PC is an Intel Core 5 120U: two fast cores and eight slow ones. The app asked for eight threads, a number chosen on a CI runner with four equal cores. Whether that costs time on this chip was never measured.

## What changed

1. **A dictation never waits for a download when another model on disk can hear it.** `engine::spare` looks for one that hears the language (Parakeet, then Whisper base, tiny, small; English also takes the models for every language). `engine::usable` and `engine::transcribe` use it, and `engine::fetch` downloads the chosen model in the background, then loads it if it is still the one chosen.
2. **The chosen model is fetched when the app starts** (`engine::upgrade`), when dictation is on or voice has been used before, and when an engine is picked in Settings. Before, it was fetched at the first words.
3. **The thread count is measured on the PC.** `whisper::tune` times a six-second test clip with 2, 4, 6 and 8 threads (those the processor has) and keeps the fewest that is within a tenth of the quickest. It runs once per start, eight seconds after the model has loaded, on a thread of its own, and steps aside while someone dictates. A model whose pass takes over 1.5 s is not measured and keeps the old count.
4. **A timing log.** `voice/timings.log` in the app's data folder gets one line per dictation (the `Timings` numbers, the model file, the thread count) and one for the thread measuring. Numbers only, never the words. Kept under 64 KB.

## Files

- `src-tauri/src/voice/engine.rs`: `spares`, `spare`, `usable`, `fetch`, `tune_soon`, the fallback in `transcribe`, `Heard.model`.
- `src-tauri/src/voice/whisper.rs`: `tune`, `candidates`, `pick`, `hiss`, `TUNED`.
- `src-tauri/src/voice/mod.rs`: `Timings.model`, `Timings.threads`, `log_line`, `newer_half`.
- `src-tauri/src/selftest/probes/voice.rs`, `tools/smoke-labels/voice.txt`: the probes below.

## Decisions the owner may want to reverse

- **A fresh install with dictation on downloads 101 MB at first start**, without asking. Dictation is on by default. Handy asks first.
- **Picking an engine in Settings starts its download at once**, in the background.
- **Fewer threads win a tie** (within a tenth). Less heat, and less to fight with whatever else is running.
- **The timing log is always on.** It holds no words, but it does show when dictation was used.

## Smoke labels

| Label | What it proves |
|---|---|
| `voiceSpare` | With Whisper small chosen and not on disk, a dictation through the real pipeline is heard by Parakeet (`Timings.model`) and the words reach the bar's field. |
| `voiceSpareFetch` | The chosen model then arrives in the background (190 MB on the runner). |
| `voiceTuned` | The thread measuring runs on the runner, keeps a count, and Parakeet still hears the fixture with it. |

Logged, not gated: `voiceSpareTimings`, `voiceTunedPass`.

In the smoke run the fetch at app start and the automatic measuring are off: the probes ask for both by hand, so no other lane's timing shares the processor with them.

## Not verified

- **Nothing here has run on a chip with fast and slow cores.** The runner has four equal ones. What `tune` keeps on the owner's PC will be in `timings.log` after the first start.
- **That the 34 s was the download.** It fits everything on the PC, but no timing was kept. The log settles it from now on.
- The fetch at app start (off in the smoke run; the same `fetch` is proven by `voiceSpareFetch`).
- A download that fails in the background: it is tried again at the next dictation or start; unit-tested nowhere.
