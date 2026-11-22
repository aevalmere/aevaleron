# rust: failure

- Workflow: rust
- Run: [45052287103](https://github.com/aevalmere/aevaleron/actions/runs/45052287103), attempt 1, on push
- Commit: bdecdcd7c8d3b2dab344f1ad9f00505452143b43
- Ref: rust/native-ui
- Conclusion: **failure**
- Failed: Unit tests
- Finished: 2026-11-22T18:14:45Z

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
| Unit tests | failure | [test.log](test.log) |
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
| installerKB | 8525 |
| upgrade | ok:from-1.1.1 |
| processCount | 1 |
| idlePrivateMB | 39.4 |
| idleWorkingSetMB | 130.4 |
| idleCpuPct | 0.16 |
| idleThreads | 19 |
| secondLaunchMs | 28 |

errors.txt: 279 error and warning lines from the logs.
