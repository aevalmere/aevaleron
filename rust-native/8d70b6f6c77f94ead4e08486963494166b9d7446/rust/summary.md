# rust: success

- Workflow: rust
- Run: [45094370364](https://github.com/aevalmere/aevaleron/actions/runs/45094370364), attempt 1, on push
- Commit: 8d70b6f6c77f94ead4e08486963494166b9d7446
- Ref: rust/native
- Conclusion: **success**
- Finished: 2026-11-23T01:41:58Z

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
| exeKB | 28718 |
| installerKB | 8599 |
| upgrade | ok:from-1.1.1 |
| processCount | 1 |
| idlePrivateMB | 38.6 |
| idleWorkingSetMB | 130.3 |
| idleCpuPct | 0.23 |
| idleThreads | 19 |
| secondLaunchMs | 25 |

errors.txt: 274 error and warning lines from the logs.
