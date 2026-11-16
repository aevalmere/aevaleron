# rust: cancelled

- Workflow: rust
- Run: [43782548928](https://github.com/aevalmere/aevaleron/actions/runs/43782548928), attempt 1, on push
- Commit: d1181819f144803f7262a324f5d06ed77463e5c7
- Ref: rust/find-order
- Conclusion: **cancelled**
- Failed: Format
- Finished: 2026-11-16T00:49:37Z

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

errors.txt: 30 error and warning lines from the logs.
