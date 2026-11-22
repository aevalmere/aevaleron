# rust: failure

- Workflow: rust
- Run: [44980855227](https://github.com/aevalmere/aevaleron/actions/runs/44980855227), attempt 1, on push
- Commit: ffd37248b5059080ca2295534b3368a5293f8bd2
- Ref: rust/cli-providers
- Conclusion: **failure**
- Failed: Smoke
- Finished: 2026-11-22T03:57:13Z

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
| Restore NSIS toolset | success |  |
| Seed NSIS toolset | success | [nsis.log](nsis.log) |
| Save NSIS toolset | success |  |
| Build installer | success | [build.log](build.log) |
| Smoke | failure | [smoke.log](smoke.log) |
| Capture check | success | [capture.log](capture.log) |
| Desktop selfcheck | success | [desktop.log](desktop.log) |
| Upload installer | skipped |  |

errors.txt: 1 error and warning lines from the logs.
