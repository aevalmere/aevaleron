# rust: failure

- Workflow: rust
- Run: [43736298957](https://github.com/aevalmere/aevaleron/actions/runs/43736298957), attempt 1, on push
- Commit: 943aa1017ec57e27b7017c380f6af102fb7f6216
- Ref: rust-rewrite
- Conclusion: **failure**
- Failed: Clippy, Unit tests
- Finished: 2026-11-15T16:38:22Z

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
| Clippy | failure | [clippy.log](clippy.log) |
| Unit tests | failure | [test.log](test.log) |
| Restore NSIS toolset | skipped |  |
| Seed NSIS toolset | skipped |  |
| Save NSIS toolset | skipped |  |
| Build installer | skipped |  |
| Smoke | skipped |  |
| Capture check | success | [capture.log](capture.log) |
| Desktop selfcheck | success | [desktop.log](desktop.log) |
| Upload installer | skipped |  |

errors.txt: 50 error and warning lines from the logs.
