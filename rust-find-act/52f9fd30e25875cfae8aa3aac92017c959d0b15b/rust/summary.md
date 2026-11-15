# rust: failure

- Workflow: rust
- Run: [43722579793](https://github.com/aevalmere/aevaleron/actions/runs/43722579793), attempt 1, on push
- Commit: 52f9fd30e25875cfae8aa3aac92017c959d0b15b
- Ref: rust/find-act
- Conclusion: **failure**
- Failed: Format, Clippy, Capture check, Desktop selfcheck
- Finished: 2026-11-15T14:16:03Z

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

errors.txt: 143 error and warning lines from the logs.
