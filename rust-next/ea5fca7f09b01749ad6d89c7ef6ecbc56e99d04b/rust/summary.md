# rust: cancelled

- Workflow: rust
- Run: [43754732956](https://github.com/aevalmere/aevaleron/actions/runs/43754732956), attempt 1, on push
- Commit: ea5fca7f09b01749ad6d89c7ef6ecbc56e99d04b
- Ref: rust/next
- Conclusion: **cancelled**
- Failed: Format
- Finished: 2026-11-15T20:00:32Z

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
| Clippy | success | [clippy.log](clippy.log) |
| Unit tests | cancelled | [test.log](test.log) |
| Restore NSIS toolset | skipped |  |
| Seed NSIS toolset | skipped |  |
| Save NSIS toolset | skipped |  |
| Build installer | skipped |  |
| Smoke | skipped |  |
| Capture check | skipped |  |
| Desktop selfcheck | skipped |  |
| Upload installer | skipped |  |

errors.txt: 19 error and warning lines from the logs.
