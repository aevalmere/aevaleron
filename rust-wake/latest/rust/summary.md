# rust: failure

- Workflow: rust
- Run: [44029992994](https://github.com/aevalmere/aevaleron/actions/runs/44029992994), attempt 1, on push
- Commit: 6448e285a1552efd03b3caf20154b30576e7c36b
- Ref: rust/wake
- Conclusion: **failure**
- Failed: Format, Unit tests, Capture check, Desktop selfcheck
- Finished: 2026-11-17T06:27:03Z

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
| Unit tests | failure | [test.log](test.log) |
| Restore NSIS toolset | skipped |  |
| Seed NSIS toolset | skipped |  |
| Save NSIS toolset | skipped |  |
| Build installer | skipped |  |
| Smoke | skipped |  |
| Capture check | failure | [capture.log](capture.log) |
| Desktop selfcheck | failure | [desktop.log](desktop.log) |
| Upload installer | skipped |  |

errors.txt: 132 error and warning lines from the logs.
