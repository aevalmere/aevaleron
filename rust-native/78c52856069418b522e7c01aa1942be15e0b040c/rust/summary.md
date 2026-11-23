# rust: success

- Workflow: rust
- Run: [45107158970](https://github.com/aevalmere/aevaleron/actions/runs/45107158970), attempt 1, on push
- Commit: 78c52856069418b522e7c01aa1942be15e0b040c
- Ref: rust/native
- Conclusion: **success**
- Finished: 2026-11-23T03:56:35Z

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
| exeKB | 28722 |
| installerKB | 8600 |
| upgrade | ok:from-1.1.1 |
| processCount | 1 |
| idlePrivateMB | 38.5 |
| idleWorkingSetMB | 130.4 |
| idleCpuPct | 0.39 |
| idleThreads | 22 |
| secondLaunchMs | 22 |

errors.txt: 274 error and warning lines from the logs.
