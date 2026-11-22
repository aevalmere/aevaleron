# rust: cancelled

- Workflow: rust
- Run: [44979419298](https://github.com/aevalmere/aevaleron/actions/runs/44979419298), attempt 1, on push
- Commit: 6dddc4a9ae233e0a88003d826c081314085f07eb
- Ref: rust/cli-providers
- Conclusion: **cancelled**
- Failed: Format
- Finished: 2026-11-22T03:12:20Z

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

errors.txt: 143 error and warning lines from the logs.
