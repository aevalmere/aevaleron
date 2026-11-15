# rust: failure

- Workflow: rust
- Run: [43722564823](https://github.com/aevalmere/aevaleron/actions/runs/43722564823), attempt 1, on push
- Commit: 9b49db851e60f43d303330a83c5ffef316bfe9c6
- Ref: rust/region-text
- Conclusion: **failure**
- Failed: Format, Clippy, Capture check, Desktop selfcheck
- Finished: 2026-11-15T14:18:18Z

| Step | Outcome | Log |
|---|---|---|
| Checkout | success |  |
| Node | success |  |
| Rust toolchain | success |  |
| Voice build flags | success |  |
| Cache cargo | success |  |
| Gate helper | success |  |
| Prebuild desktop helper | success | [prebuild.log](prebuild.log) |
| Format | failure | [fmt.log](fmt.log) |
| Format patch | success | [fmt.patch](fmt.patch) |
| Clippy | failure | [clippy.log](clippy.log) |
| Unit tests | success | [test.log](test.log) |
| Restore NSIS toolset | skipped |  |
| Seed NSIS toolset | skipped |  |
| Save NSIS toolset | skipped |  |
| Build installer | skipped |  |
| Smoke | skipped |  |
| Capture check | failure | [capture.log](capture.log) |
| Desktop selfcheck | failure | [desktop.log](desktop.log) |
| Upload installer | skipped |  |

errors.txt: 76 error and warning lines from the logs.
