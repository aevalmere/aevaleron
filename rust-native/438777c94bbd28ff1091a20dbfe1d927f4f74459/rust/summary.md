# rust: success

- Workflow: rust
- Run: [45063829913](https://github.com/aevalmere/aevaleron/actions/runs/45063829913), attempt 1, on push
- Commit: 438777c94bbd28ff1091a20dbfe1d927f4f74459
- Ref: rust/native
- Conclusion: **success**
- Finished: 2026-11-22T20:34:08Z

| Step | Outcome | Log |
|---|---|---|
| Checkout | success |  |
| Node | success |  |
| Rust toolchain | success |  |
| Voice build flags | success |  |
| Cache cargo | success |  |
| Gate helper | success |  |
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
| exeKB | 28647 |
| installerKB | 8567 |
| upgrade | ok:from-1.1.1 |
| processCount | 1 |
| idlePrivateMB | 39 |
| idleWorkingSetMB | 130.2 |
| idleCpuPct | 0.31 |
| idleThreads | 20 |
| secondLaunchMs | 49 |

errors.txt: 274 error and warning lines from the logs.
