# rust: failure

- Workflow: rust
- Run: [44031102784](https://github.com/aevalmere/aevaleron/actions/runs/44031102784), attempt 1, on push
- Commit: 19cd3887bacf84022712f7f1ce7b82c298a464b4
- Ref: rust/ask-do
- Conclusion: **failure**
- Failed: Format, Clippy, Capture check, Desktop selfcheck
- Finished: 2026-11-17T06:34:26Z

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

errors.txt: 50 error and warning lines from the logs.
