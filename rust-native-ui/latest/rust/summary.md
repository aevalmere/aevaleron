# rust: failure

- Workflow: rust
- Run: [45037271159](https://github.com/aevalmere/aevaleron/actions/runs/45037271159), attempt 1, on push
- Commit: bab8a3a10550c7054dce4befa30013e248dbc5bc
- Ref: rust/native-ui
- Conclusion: **failure**
- Failed: Smoke
- Finished: 2026-11-22T15:43:48Z

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
| Save NSIS toolset | success |  |
| Build installer | success | [installer.log](installer.log) |
| Smoke | failure | [smoke.log](smoke.log) |
| Capture check | success | [capture.log](capture.log) |
| Desktop selfcheck | success | [desktop.log](desktop.log) |
| Install check | success | [install.log](install.log) |
| Upload installer | success |  |
| Upload exe | success |  |

| Metric | Value |
|---|---|
| exeKB | 28216 |
| installerKB | 8484 |
| upgrade | ok:from-1.1.1 |
| processCount | 1 |
| idlePrivateMB | 42.1 |
| idleWorkingSetMB | 133.7 |
| idleCpuPct | 0.2 |
| idleThreads | 19 |
| secondLaunchMs | 19 |

errors.txt: 277 error and warning lines from the logs.
