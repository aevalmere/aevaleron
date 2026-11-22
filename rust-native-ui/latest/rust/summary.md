# rust: success

- Workflow: rust
- Run: [45054412210](https://github.com/aevalmere/aevaleron/actions/runs/45054412210), attempt 1, on push
- Commit: 415e6c02d29a702f72e89d5fd1d39d8bbc40cc60
- Ref: rust/native-ui
- Conclusion: **success**
- Finished: 2026-11-22T18:36:41Z

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
| exeKB | 28388 |
| installerKB | 8529 |
| upgrade | ok:from-1.1.1 |
| processCount | 1 |
| idlePrivateMB | 38 |
| idleWorkingSetMB | 129.9 |
| idleCpuPct | 0.08 |
| idleThreads | 20 |
| secondLaunchMs | 22 |

errors.txt: 274 error and warning lines from the logs.
