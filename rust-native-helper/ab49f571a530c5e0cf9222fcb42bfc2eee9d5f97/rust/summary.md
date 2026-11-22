# rust: failure

- Workflow: rust
- Run: [44980229878](https://github.com/aevalmere/aevaleron/actions/runs/44980229878), attempt 1, on push
- Commit: ab49f571a530c5e0cf9222fcb42bfc2eee9d5f97
- Ref: rust/native-helper
- Conclusion: **failure**
- Failed: Format, Clippy, Unit tests, Capture check, Desktop selfcheck
- Finished: 2026-11-22T03:34:10Z

| Step | Outcome | Log |
|---|---|---|
| Checkout | success |  |
| Node | success |  |
| Rust toolchain | success |  |
| Voice build flags | success |  |
| Cache cargo | success |  |
| Gate helper | success |  |
| Format | failure | [fmt.log](fmt.log) |
| Format patch | success | [fmt.patch](fmt.patch) |
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

errors.txt: 606 error and warning lines from the logs.
