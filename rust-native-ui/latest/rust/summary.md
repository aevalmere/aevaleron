# rust: failure

- Workflow: rust
- Run: [45050024359](https://github.com/aevalmere/aevaleron/actions/runs/45050024359), attempt 1, on push
- Commit: d07b3b940cf0405014a7bae5c2fb337b7423c9dd
- Ref: rust/native-ui
- Conclusion: **failure**
- Failed: Smoke
- Finished: 2026-11-22T17:46:43Z

| Step | Outcome | Log |
|---|---|---|
| Checkout | success |  |
| Node | success |  |
| Rust toolchain | success |  |
| Voice build flags | success |  |
| Cache cargo | success |  |
| Gate helper | success |  |
| Prebuild desktop helper | success | [prebuild.log](prebuild.log) |
| Format | success |  |
| Format patch | skipped |  |
| Clippy | success | [clippy.log](clippy.log) |
| Unit tests | success | [test.log](test.log) |
| Release build | success | [build.log](build.log) |
| Restore NSIS toolset | success |  |
| Seed NSIS toolset | success | [nsis.log](nsis.log) |
| Save NSIS toolset | skipped |  |
| Build installer | success | [installer.log](installer.log) |
| Smoke | failure | [smoke.log](smoke.log) |
| Capture check | success | [capture.log](capture.log) |
| Desktop selfcheck | success | [desktop.log](desktop.log) |
| Install check | success | [install.log](install.log) |
| Upload installer | success |  |
| Upload exe | success |  |

| Metric | Value |
|---|---|
| exeKB | 28388 |
| installerKB | 8526 |
| upgrade | ok:from-1.1.1 |
| processCount | 1 |
| idlePrivateMB | 38.9 |
| idleWorkingSetMB | 130.2 |
| idleCpuPct | 0.16 |
| idleThreads | 18 |
| secondLaunchMs | 20 |

errors.txt: 278 error and warning lines from the logs.
