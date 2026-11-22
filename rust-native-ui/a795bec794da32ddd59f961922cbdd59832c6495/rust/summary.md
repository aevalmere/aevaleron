# rust: success

- Workflow: rust
- Run: [45041909944](https://github.com/aevalmere/aevaleron/actions/runs/45041909944), attempt 1, on push
- Commit: a795bec794da32ddd59f961922cbdd59832c6495
- Ref: rust/native-ui
- Conclusion: **success**
- Finished: 2026-11-22T16:33:49Z

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
| Smoke | success | [smoke.log](smoke.log) |
| Capture check | success | [capture.log](capture.log) |
| Desktop selfcheck | success | [desktop.log](desktop.log) |
| Install check | success | [install.log](install.log) |
| Upload installer | success |  |
| Upload exe | success |  |

| Metric | Value |
|---|---|
| exeKB | 28387 |
| installerKB | 8525 |
| upgrade | ok:from-1.1.1 |
| processCount | 1 |
| idlePrivateMB | 38.9 |
| idleWorkingSetMB | 130.1 |
| idleCpuPct | 0.23 |
| idleThreads | 18 |
| secondLaunchMs | 19 |

errors.txt: 274 error and warning lines from the logs.
