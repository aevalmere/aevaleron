# rust: failure

- Workflow: rust
- Run: [45060306109](https://github.com/aevalmere/aevaleron/actions/runs/45060306109), attempt 1, on push
- Commit: 61c47fdbd2fd30b23568f49e5959d74c60695e50
- Ref: rust/native
- Conclusion: **failure**
- Failed: Unit tests
- Finished: 2026-11-22T19:53:42Z

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
| Unit tests | failure | [test.log](test.log) |
| Release build | success | [build.log](build.log) |
| Restore NSIS toolset | success |  |
| Seed NSIS toolset | success | [nsis.log](nsis.log) |
| Save NSIS toolset | success |  |
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
| idlePrivateMB | 38.5 |
| idleWorkingSetMB | 130.2 |
| idleCpuPct | 0.08 |
| idleThreads | 20 |
| secondLaunchMs | 23 |

errors.txt: 278 error and warning lines from the logs.
