# rust: cancelled

- Workflow: rust
- Run: [44979550594](https://github.com/aevalmere/aevaleron/actions/runs/44979550594), attempt 1, on push
- Commit: 2f6be157bdae8100c5af86ff05e66c2cc3b6a44b
- Ref: rust/cli-providers
- Conclusion: **cancelled**
- Failed: Format
- Finished: 2026-11-22T03:13:09Z

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
| Clippy | cancelled | [clippy.log](clippy.log) |
| Unit tests | skipped |  |
| Restore NSIS toolset | skipped |  |
| Seed NSIS toolset | skipped |  |
| Save NSIS toolset | skipped |  |
| Build installer | skipped |  |
| Smoke | skipped |  |
| Capture check | skipped |  |
| Desktop selfcheck | skipped |  |
| Upload installer | skipped |  |

errors.txt: 150 error and warning lines from the logs.
