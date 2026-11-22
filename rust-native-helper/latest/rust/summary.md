# rust: failure

- Workflow: rust
- Run: [44982479542](https://github.com/aevalmere/aevaleron/actions/runs/44982479542), attempt 1, on push
- Commit: 20b61466d8759129af5170ec770b5ad6ac2c4649
- Ref: rust/native-helper
- Conclusion: **failure**
- Failed: Smoke, Desktop selfcheck
- Finished: 2026-11-22T04:18:34Z

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
| Clippy | success | [clippy.log](clippy.log) |
| Unit tests | success | [test.log](test.log) |
| Restore NSIS toolset | success |  |
| Seed NSIS toolset | success | [nsis.log](nsis.log) |
| Save NSIS toolset | success |  |
| Build installer | success | [build.log](build.log) |
| Smoke | failure | [smoke.log](smoke.log) |
| Capture check | success | [capture.log](capture.log) |
| Desktop selfcheck | failure | [desktop.log](desktop.log) |
| Upload installer | skipped |  |

errors.txt: 0 error and warning lines from the logs.
