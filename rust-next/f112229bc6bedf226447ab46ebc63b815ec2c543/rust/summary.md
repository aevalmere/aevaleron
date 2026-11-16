# rust: failure

- Workflow: rust
- Run: [43782660910](https://github.com/aevalmere/aevaleron/actions/runs/43782660910), attempt 1, on push
- Commit: f112229bc6bedf226447ab46ebc63b815ec2c543
- Ref: rust/next
- Conclusion: **failure**
- Failed: Format, Clippy
- Finished: 2026-11-16T00:54:19Z

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
| Capture check | success | [capture.log](capture.log) |
| Desktop selfcheck | success | [desktop.log](desktop.log) |
| Upload installer | skipped |  |

errors.txt: 30 error and warning lines from the logs.
