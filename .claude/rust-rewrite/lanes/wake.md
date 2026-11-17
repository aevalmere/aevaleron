# Lane brief: wake

Branch `rust/wake`, cut from `rust-rewrite` at 0c3f8e8. Not merged, not released.

## Goal

The owner asked for two things. First, Whisper base becomes the default voice engine. Second, an optional hands-free mode: say the name ("Aevaleron" by default), say what you want, and stop talking. The line is then sent as if it had been typed in the bar and Enter pressed. The mode is off until the person turns it on, in Settings or in the tray. A mark shows while it listens. The idle mode must cost next to nothing, keep no audio on disk and send nothing while it waits.

## Files

New:
- `src-tauri/src/voice/wake.rs`: the state machine, the listening thread, the checker thread, `submit`.
- `src-tauri/src/voice/keyword.rs`: the name matcher, the stripping, `check_word`.

Edited:
- `src-tauri/src/voice/`: `settings.rs`, `vad.rs` (`Gate`), `capture.rs` (`Ring`, `listen`, `listen_queue`), `mod.rs` (`busy`, `claim_pill`, `Writing`), `engine.rs`, `whisper.rs` (`TUNE_STOP_MS`), `ipc.rs`.
- `src-tauri/src/tray.rs`: the switch and the dot. Several small blocks, each marked `wake lane`.
- `settings/settings.html`: one new section, plus Whisper base moved first in the engine list.
- `renderer/voice-settings.js`.
- `src-tauri/src/selftest/probes/voice.rs`.
- `tools/smoke-labels/voice.txt`: 8 new labels.
- `tools/ui-shots/lanes/voice.mjs`: 3 new checks and 1 new shot.

Shared counts: none changed. `selftest/probes/mod.rs` LANES stays at 14, and `settings/settings.js` is not touched.

## Decisions

**D-wake-1: Whisper base is the default, and stored defaults move to it.**
- An unmarked `parakeet-110m` (or one stored with `enginePicked:false`) reads as no choice and becomes `whisper-base`.
- An unmarked `whisper-base` is also no choice.
- A picked engine is kept, and nothing flips back.
- The model is not bundled into the installer, because that adds 64 MB to every update. A fresh install fetches base.en at first start, as before.
- Until base.en arrives, a dictation runs on a spare that is already on disk (Parakeet or tiny, `engine::spares`), so no dictation waits for a download.
- Parakeet files already on disk are kept, and nothing else is fetched.
- `whisper::TUNE_STOP_MS` goes from 1500 to 2500 ms, because a Whisper base pass takes about 0.9 s.

**D-wake-2: listening.**
- `capture::listen` keeps a 2 s mono ring with no allocation in the audio callback.
- Every 30 ms the thread reads whole 20 ms frames into a `vad::Gate`. The gate is an energy gate whose noise floor follows the room: it drops at once and rises slowly, so steady noise or music becomes the floor within about ten seconds.
- No model runs during silence or steady noise.
- The ring is dropped when the mode goes off, when the microphone goes away, or when the thread stops.
- Hold-to-talk and the bar's mic come first (`voice::busy()`): the machine resets and ignores the sound until they finish.
- A device that errors or stops sending (after sleep, or when unplugged) gives a clear status line and a retry every 2 s, doubling up to 30 s. A missing model gives "waiting" and a recheck every 5 s. A panic gives an error status, never a crash.

**D-wake-3: the check.**
- After speech starts, the first 1.8 s is checked, or less at a 160 ms pause.
- Only one check runs at a time, on its own thread with its model loaded ahead.
- After 4 failed checks within 60 s, checking rests for 30 s, then 60 s, up to 120 s.
- Model order: Parakeet 110M if it is on disk and the language is English, then Whisper tiny, then the dictation model. Nothing is downloaded for the check.
- This order comes from the bench below. Parakeet took 45 to 68 ms against about 400 ms for tiny and about 800 ms for base, and base missed the name at the start of a sentence even with the name as its prompt.

**D-wake-4: matching (`keyword.rs`).**
- Words are compared on a sound key: vowels are one sound, c/k/q are one, z/s are one, ph/f are one, and repeats collapse.
- Up to two leading fillers are allowed (hey, ok, okay, um and the like).
- Strict mode, used by the check: names up to 4 sounds allow 0 edits, 5 to 7 sounds allow 1, longer names allow 2. The consonant count must also match, with at most one consonant different.
- So "Evelloron" (Parakeet's spelling) matches.
- "Avalon" does not match: it has no r, and a name the person did not say must not wake it. It is accepted only by the looser strip of the final transcript, once the check has already heard the name.
- `check_word` refuses names that are too short, too long, made of fillers, or not letters.

**D-wake-5: timings.**
- The pill shows as soon as the check says yes.
- After the bare name, the person has 2.5 s to start speaking.
- The input ends after 900 ms of quiet. Hold-to-talk users pause mid-sentence for 400 to 700 ms, and 900 ms rarely cuts a thought. Each extra 100 ms is felt on every line. It is capped at 30 s.
- With nothing left after the name, nothing is sent and the pill hides.

**D-wake-6: the line goes through `voice::wake::submit(app, line)`** (`src-tauri/src/voice/wake.rs`).
- It opens the bar showing the line and calls `host::ask::submit` with `inferred: true`.
- On success it clears the field and records the line in history, as a typed send does.
- The rust/ask-do lane replaces only this body.
- Before submit, the transcript gets: the dictation engine with the name as the Whisper prompt, `strip_noise`, `keyword::strip`, and `tidy`.

**D-wake-7: the mark.**
- A red dot with a clear ring sits at the lower right of the tray icon while the microphone is open for the name.
- The tooltip adds "listening for Aevaleron".
- Settings says plainly that the microphone stays open, that a red dot shows, and that nothing is saved or sent while it waits.

## Numbers (smoke, run 44031428703, 5bde87d)

| What | Result |
|---|---|
| Idle, 11 s of silence | 0.00% of one core on the listening thread, 2 us a pass, 0 checks |
| End of name to pill | 200 ms (59 ms check on Parakeet) |
| End of speech to sent line | 1735 ms: 900 ms of quiet, then about 835 ms of Whisper base transcription |
| Whisper base dictation, 17 s utterance, release to text | 868 ms (budget 1800 ms) |
| Parakeet, same utterance | 239 ms |

Check bench on SAPI speech of "Aevaleron":

| Model | Name alone | First 1.8 s of "Aevaleron, what is the weather tomorrow?" |
|---|---|---|
| base.en | 823 ms, yes | 783 ms, missed |
| tiny.en | 397 ms, yes | 407 ms, yes |
| Parakeet | 45 ms, yes | 68 ms, yes |

All new gates passed: voiceWakeListens, Idle, OneBreath, Pause, Ignores, Bare, Yields, Off.

## Not verified

- **No real microphone or real voice.** CI plays Windows SAPI speech into the listening at real time through a test source in place of cpal. Real rooms, accents, a real cpal stream, and device unplug or sleep recovery were not exercised. The code paths for those exist, but nothing has run them.
- **Idle cost covers the listening thread only.** The real cpal callback thread is not counted. GetThreadTimes ticks in about 15.6 ms steps, so "0.00%" means under about 0.15%.
- **The SAPI voice pronounces the name one way.** Parakeet writes it "Evelloron". How other people say it is untested.
- **Without Parakeet on disk, the pill takes longer.** On an English install without it (fresh installs fetch only base.en), the check runs on tiny if present, else base. Then the pill takes about 0.4 to 0.9 s, not 0.2 s, and base can miss the name at the start of a sentence.
- **The real `submit` path was not run.** The smoke test uses a sink: the bar opening and the ask itself are not driven by this lane's probe.
- **Not checked on the app's real UI:** the tray dot on a real taskbar, and the CheckMenuItem.
