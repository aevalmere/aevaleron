# rust: failure

- Workflow: rust
- Run: [43722582036](https://github.com/aevalmere/aevaleron/actions/runs/43722582036), attempt 1, on push
- Commit: f615a76bda22c8a7e8635795e48b414127a6eb6d
- Ref: rust/private-apps
- Conclusion: **failure**
- Failed: Format, Capture check, Desktop selfcheck
- Finished: 2026-11-15T14:21:15Z

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
| Unit tests | success | [test.log](test.log) |
| Restore NSIS toolset | skipped |  |
| Seed NSIS toolset | skipped |  |
| Save NSIS toolset | skipped |  |
| Build installer | skipped |  |
| Smoke | skipped |  |
| Capture check | failure | [capture.log](capture.log) |
| Desktop selfcheck | failure | [desktop.log](desktop.log) |
| Upload installer | skipped |  |

errors.txt: 102 error and warning lines from the logs.
