# Lane brief: voice-transcribe

Branch `rust/voice-transcribe`, cut from `rust/voice-one-pass` (f35b788). Not merged, not released. Two rounds: the first moved speech to text to transcribe.cpp and added Parakeet as a choice; the second, after the owner read the first report, made Parakeet 110M the default for English, added the tidy and Parakeet 0.6B, and moved the speed gates.

## Goal

Move dictation's speech to text from whisper.cpp (`whisper-rs`) to transcribe.cpp (`transcribe-cpp` 0.4.2, the library Handy uses), so Parakeet can run beside Whisper. The two libraries cannot link into one exe: each brings its own ggml (LNK2005, `tools/voice-bench` on branch `bench/parakeet`).

## Files

`src-tauri/Cargo.toml` (the voice block), `src-tauri/build.rs`, `src-tauri/src/voice/{whisper,model,engine,stream,settings,cleanup,ipc}.rs`, `src-tauri/src/voice/whisper.cmake` (comments only), `src-tauri/src/selftest/probes/voice.rs`, `tools/voice-cmake-env.mjs` (new), `tools/smoke-labels/voice.txt`, `.github/workflows/{rust,release}.yml`, `settings/settings.html`, `renderer/voice-settings.js`, `tools/ui-shots/lanes/voice.mjs`. `src-tauri/.cargo/config.toml` is deleted: it only set `CMAKE_PROJECT_INCLUDE` for whisper-rs-sys.

## Decisions

**D-voice-16: one library, transcribe.cpp, for Whisper and Parakeet.** `voice/whisper.rs` is still the only file that talks to the engine. A loaded model keeps one session. No timestamps. Whisper runs one greedy pass (`temperature_inc = 0`), with the text so far as its prompt; a pass with more words than the clip could hold is done again with the library's settings (`looped`, kept, Whisper only). The library stops a text that repeats or runs out of room and hands back what it had; that partial text is used.

**D-voice-17: models are GGUF files from huggingface.co/handy-computer, each pinned to a commit and a SHA-256** (`model.rs`, read 2026-11-14). Whisper tiny, base and small at Q5_K_M, English and multilingual; `parakeet-tdt_ctc-110m-Q5_K_M.gguf` (101 MB); `parakeet-tdt-0.6b-v3-Q5_K_M.gguf` (549 MB). A whisper.cpp `.bin` an earlier build fetched still loads in transcribe.cpp, so it runs until its GGUF has arrived in the background, then it is deleted (`model::legacy_for`, `engine::upgrade`). If such a file does not load, the GGUF is fetched at once.

**D-voice-18: the build flags reach the library through `TRANSCRIBE_CMAKE_ARGS`.** `whisper.cmake` is unchanged in what it sets (static C++ runtime, AVX2, FMA, F16C, no AVX-512, `GGML_NATIVE` off). cargo's `[env]` table cannot write an absolute path inside a `-D` argument, so `tools/voice-cmake-env.mjs` sets the variable in `rust.yml` and `release.yml`, and `build.rs` stops a build that lacks it. `voiceNoCppRuntime` is the proof the file was applied.

**D-voice-19: engine `parakeet-110m`.** English only: with another language chosen, Whisper base stands in, which keeps the Language setting true (`engine::spec`). Parakeet is handed each piece whole, silence left on (`engine::piece`): cut to the speech as Whisper's pieces are, it wrote the fixture's "tomorrow" as "to morrow" and left out every comma (`voiceSpeedParakeetTrimmed`).

**D-voice-20: `parakeet-110m` is the default engine, and a choice is marked** (`settings.rs`). Every earlier build stored `"engine": "whisper-base"` whether the person chose it or not, so the file cannot tell a choice from the old default. The settings now carry `enginePicked`, written true when Settings changes the engine. A stored `whisper-base` without the mark reads as not chosen and becomes Parakeet 110M; any other stored engine without the mark was a choice (it was never a default) and is kept. So someone who had deliberately stayed on "On this PC" moves to Parakeet once; picking Whisper base again sticks.

**D-voice-21: every utterance is tidied in Rust when AI cleanup is off or fails** (`cleanup::tidy`, no setting, all local engines and Gemini's text too, since it sits in the pipeline after the engine). Three rules:

- A hesitation standing alone as a word goes, with the comma after it: um, umm, uh, uhh, uhm, erm, and hmm and mhm, which the earlier tidy already dropped. A comma before it stays. A word that only starts like one (umbrella) is a word.
- A capital on the first letter, and on a sentence whose opening hesitation went ("... tomorrow. Um, so we" gives "... tomorrow. So we").
- A full stop at the end unless the utterance ends with `.`, `?`, `!`, `:` or `…` (closing quotes and brackets are looked past). A trailing comma or semicolon is replaced by it. Text ending in Hindi, Thai, Chinese or Japanese characters gets none: those scripts have a stop of their own.

Nothing between pieces joined after a pause is changed. The AI cleanup's own output is not tidied again.

**D-voice-22: engine `parakeet-0.6b`, Parakeet TDT 0.6B v3** (`handy-computer/parakeet-tdt-0.6b-v3-gguf` at d8d8c32, the Q5_K_M file `voice-bench.yml` used; 548,946,272 bytes, which is 549 MB and not the 450 MB the brief gave). It runs for English, `auto` and the seven other languages Settings offers that it covers: Spanish, French, German, Italian, Portuguese, Dutch, Polish. For Turkish, Hindi, Japanese, Korean and Chinese, Whisper base stands in (`engine::spec`, `engine::stand_in`), and Settings says so on that engine's line. Like the 110M it is handed each piece whole.

**D-voice-23: Settings, Voice.** Each engine is named and has one line: Parakeet 110M "On this PC. Fastest. English, with plainer punctuation."; Whisper base "On this PC. Slower, with fuller punctuation."; Whisper tiny "On this PC. Small download, gets more words wrong."; Whisper small "On this PC. More accurate than base, slower on older PCs."; Parakeet 0.6B "On this PC. Best text. Large download, slower without a strong processor."; Gemini unchanged. The app had no place that lists notices, so the attribution is a row at the foot of the Voice page, "Speech models" (`#sec-voice-credits`): "Parakeet is by NVIDIA, used under CC BY 4.0 (creativecommons.org/licenses/by/4.0) and converted to GGUF by Handy. Whisper is by OpenAI, under the MIT licence."

**D-voice-24: the speed gates time the default engine.** `voiceSpeed`: Parakeet 110M on the fixture, the better of two passes, 400 ms. `voiceSpeedE2e`: release to text of the 17 s utterance with the default engine, 600 ms. `voiceSpeedWhisper` (new): Whisper base on the fixture, 1400 ms. The two fixture gates are a little over 1.4 times what the slower of the two runner processors took (below). The utterance varied more on that processor (315 and 374 ms), so its gate is 1.6 times the slower of those; it still fails a build that falls back to Whisper (832 ms and up). A time over its budget is taken again after five seconds, twice at most, and the best counts (`settled` in the probe; `voiceSpeed` logs the first time beside the best, `voiceSpeedE2e` logs `tries`): run 43655509204 failed `voiceSpeed` at 592 ms because the runner was busy for about ten seconds, every pass in that stretch two to five times its usual, and the same build had passed twice. A build that is slow stays slow through the retries. Whisper base's 17 s utterance is logged, not gated.

## Smoke labels

Gated: `voiceParakeet` (Parakeet downloads, loads and hears the fixture's "tomorrow"), `voiceSpeed`, `voiceSpeedE2e`, `voiceSpeedWhisper`. Logs: `voiceSpeedAfter`, `voiceSpeedShort`, `voiceSpeedTiny` (Whisper); `voiceSpeedParakeet`, `voiceSpeedParakeetShort`, `voiceSpeedParakeetTrimmed`, `voiceSpeedParakeetPadded`; `voiceSpeedWhisperTidy` and `voiceSpeedParakeetTidy` (the fixture's text after the tidy); `voiceSpeedTimings` and `voiceSpeedHeard` (the 17 s utterance, default engine, the text as it reached the bar's field); `voiceSpeedWhisperTimings` and `voiceSpeedWhisperHeard` (the same with Whisper base); `voiceLegacyBin`, `voiceSystem`. Gone since the first round: `voiceSpeedParakeetTimings`, `voiceSpeedParakeetHeard` (now the default's own labels), and before that `voiceSpeedBefore`, `voiceSpeedBreakdown`.

ui-shots, lane `voice`: `voiceSettingsPage` expects Parakeet 110M checked; `voiceSettingsEngines` (new) expects six engine lines, exactly one saying "Fastest" and that one Parakeet 110M's, and the credits row naming NVIDIA and CC BY 4.0.

## Numbers

Runner: windows-latest, 4 vCPU, no GPU. Fixture `dictation.wav`, 7.9 s, 7.5 s once the silence is cut. Before is whisper.cpp at f35b788. The first four runs landed on an AMD EPYC 9V74: 43594967001 (42684a1), 43597308478 (3add819), 43599526371 (fb12386), 43600668620 (4f29f3c). The fifth and sixth, 43652925433 (58447bc, the second round's code) and 43654278124 (e949022), landed on an AMD EPYC 7763, which is about a fifth slower on everything (`voiceCalibrate` 202 ms against 165). A seventh, 43655509204 (37b1e43), also on the 7763, is left out of the table: Whisper tiny on four threads took 1483 ms where it takes 504, Parakeet 592 and 1004 ms on the fixture and 1405 ms on the cut clip, then 303 ms on the next pass and 334 ms for the 17 s utterance. Nothing in the app was running beside the probe that I could find; the runner was busy.

| What | Before | EPYC 9V74, four runs | EPYC 7763, two runs |
|---|---|---|---|
| Whisper base.en, fixture (`voiceSpeedAfter`) | 645 ms | 807, 830, 809, 836 ms | 980, 978 ms |
| Whisper base.en, first 3 s (`voiceSpeedShort`) | 561 ms | 707, 724, 720, 712 ms | 900 ms |
| Whisper tiny.en, fixture (`voiceSpeedTiny`) | 297 ms | 423, 435, 428, 422 ms | 518 ms |
| Whisper base.en, 17 s utterance with a pause, release to text | 695 ms | 832, 849, 862, 835 ms | 1039, 1023 ms |
| Parakeet 110M, whole fixture, first pass after the load | | 555, 251, 332 ms (from the second run) | 295, 292 ms |
| Parakeet 110M, whole fixture, second pass | | 227, 217 ms (from the third run) | 277, 274 ms |
| Parakeet 110M, fixture cut to the speech | | 203, 190, 193, 191 ms | 240 ms |
| Parakeet 110M, first 3 s | | 98, 98, 101, 97 ms | 124 ms |
| Parakeet 110M, 17 s utterance with a pause, release to text | | 195 (pieces cut), 235, 237, 235 ms | 315, 374 ms |
| Parakeet 110M, load from disk | | 71 to 73 ms | 78, 76 ms |
| Installer | 5.27 MiB | 5.93 MiB | 5.93 MiB |

Whisper is slower than it was by 130 to 190 ms on every clip. transcribe.cpp encodes Whisper's whole 30 s window whatever the clip's length and has no setting for a shorter one; whisper.cpp's `audio_ctx`, fitted to the clip, was where the earlier time came from. On the EPYC 7763 Whisper base took 978 to 980 ms, over the first round's 900 ms gate; that gate is now `voiceSpeedWhisper` at 1400 ms.

Parakeet's first pass on the whole fixture after a load is uneven on the 9V74 (555, 251, 332 ms, the extra booked under "decode" while the library's own decoder line says 7 ms); a second pass is 217 to 227 ms and the pipeline 233 to 238 ms a piece. Cause not found. The gate takes the better of two passes.

Parakeet 0.6B has one measurement, and it is not from this branch: about 1100 ms on the 7.9 s clip in the benchmark on `bench/parakeet` (EPYC 7763). Smoke does not download it.

Text, same fixture, as the engine wrote it:

- Whisper base.en: "Umm, so I think we should meet at 3, no, 4pm tomorrow, to go over the budget."
- Parakeet 110M, whole clip: "Um, so I think we should meet at three no, four PM tomorrow, to go over the budget"
- Parakeet 110M, cut to the speech: "Um so I think we should meet at three no four PM to morrow to go over the budget"
- Parakeet 110M, first 3 s: "Um so I think we should meet at three"

After the tidy (runs 43652925433 and 43654278124, the same text in both):

- Whisper base.en, fixture: "So I think we should meet at 3, no, 4pm tomorrow, to go over the budget."
- Parakeet 110M, fixture: "So I think we should meet at three no, four PM tomorrow, to go over the budget."
- Whisper base.en, 17 s utterance: "So I think we should meet at 3, no, 4pm tomorrow, to go over the budget. So I think we should meet at 3, no, 4pm tomorrow, to go over the budget."
- Parakeet 110M, 17 s utterance: "So I think we should meet at three no, four PM tomorrow, to go over the budget so I think we should meet at three no four PM tomorrow to go over the budget."

First download: Parakeet 110M 101 MB, Whisper base 64 MB (was 60), tiny 44 MB (was 32), small 194 MB (was 190), Parakeet 0.6B 549 MB.

## What still works, and the gaps

`PARITY.md` has no voice rows (voice is newer than the Electron build it describes) and is unchanged. Against the behaviour the lane was told to keep:

| Behaviour | State |
|---|---|
| Pieces transcribed at pauses while the key is held | kept, both engines (`voiceSpeedTimings`, `voiceSpeedWhisperTimings`: 1 piece early) |
| The text so far as the prompt, for casing across pieces | kept for Whisper. **Gap: Parakeet takes no prompt.** Each piece is heard on its own, and the tidy leaves the join alone: in the 17 s run the second piece begins lowercase with no full stop before it, and has no commas |
| Old processors refused (`cpu_ok`: AVX2, FMA, F16C) | kept, all engines; Settings' message no longer says Whisper |
| Unload ten minutes after the last use; kept while dictation is on | kept (`whisper.rs`, same code path) |
| Language setting | kept for Whisper (named only to a multilingual model; `auto` left to the model). Parakeet 110M is English only and Parakeet 0.6B covers eight of the thirteen languages offered; for the rest Whisper base runs and Settings says so |
| Short encoder window for Whisper | **Gone.** No such setting in transcribe.cpp; this is the 130 to 190 ms above |
| Files earlier builds fetched | run until replaced, then deleted |

## Not verified

- No run on a real PC or with a microphone; every recording is the fixture, on the CI runner.
- Parakeet 0.6B was never downloaded or run on this branch. Its size and SHA-256 are Hugging Face's own for the pinned commit; that it loads in this build, how it takes a language other than English, and its text on the fixture are all unproven here. The first person to pick it is the test.
- The move of an existing install to the new default: `from_value` has unit tests, but no settings file from a real earlier install was read. Such an install fetches Parakeet's 101 MB on its first dictation after the update, with the download shown as for any model; that path was not run from an old install.
- The tidy on real dictation: unit tests and the one fixture. A language other than English was not run through it.
- An old processor: `cpu_ok` is unchanged and the flags file is applied, but nothing ran on a PC without AVX2 or on one with AVX-512.
- Of the earlier builds' files only `ggml-tiny.en-q5_1.bin` was loaded (`voiceLegacyBin`); the base, small and f16 files go the same way but were not run. The background swap from an old file to its GGUF was not exercised.
- The multilingual and small GGUF files: their hashes are Hugging Face's own, none was downloaded here.
- `release.yml` did not run (no release).
- Parakeet's accuracy beyond the one fixture; the benchmark on `bench/parakeet` has more clips.
- The speed budgets come from two runs on the EPYC 7763 and four on the 9V74. A runner slower than the 7763 by more than 40% would fail them. The 17 s utterance with Parakeet moved from 315 to 374 ms between two runs on the same processor, cause not looked into; its gate was widened from 450 to 600 ms for that.
- What kept the runner busy in run 43655509204 is not known. The retry was not seen to fire: the run after it had no slow stretch.

## For the owner

- Parakeet 110M joins two pieces with no full stop or capital between them, because it takes no prompt and the tidy was told to leave joins alone. If long dictations with pauses read badly, a rule for the join (a full stop and a capital where the earlier piece ended a pause) is the next step.
- Someone who had chosen Whisper base on purpose is moved to Parakeet once (D-voice-20).
- The credits row says Whisper is under the MIT licence, which is OpenAI's licence for the model; Handy's GGUF repositories for Whisper are marked Apache-2.0. If the conversion's licence should be named too, the row is one line in `settings/settings.html`.
- Whisper stays about 160 ms slower than it was under whisper.cpp; nothing in this library gets it back.
