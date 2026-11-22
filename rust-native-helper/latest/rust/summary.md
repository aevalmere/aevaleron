# rust: failure

- Workflow: rust
- Run: [44981380936](https://github.com/aevalmere/aevaleron/actions/runs/44981380936), attempt 1, on push
- Commit: 2e1985ad396d243fdd499bdfc966b01e8ab5a8d3
- Ref: rust/native-helper
- Conclusion: **failure**
- Failed: Clippy, Unit tests, Capture check, Desktop selfcheck
- Finished: 2026-11-22T03:49:45Z

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
| Clippy | failure | [clippy.log](clippy.log) |
| Unit tests | failure | [test.log](test.log) |
| Restore NSIS toolset | skipped |  |
| Seed NSIS toolset | skipped |  |
| Save NSIS toolset | skipped |  |
| Build installer | skipped |  |
| Smoke | skipped |  |
| Capture check | failure | [capture.log](capture.log) |
| Desktop selfcheck | failure | [desktop.log](desktop.log) |
| Upload installer | skipped |  |

errors.txt: 48 error and warning lines from the logs.
