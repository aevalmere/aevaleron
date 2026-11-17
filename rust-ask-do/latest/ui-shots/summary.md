# ui-shots: failure

- Workflow: ui-shots
- Run: [44031102841](https://github.com/aevalmere/aevaleron/actions/runs/44031102841), attempt 1, on push
- Commit: 19cd3887bacf84022712f7f1ce7b82c298a464b4
- Ref: rust/ask-do
- Conclusion: **failure**
- Failed: Screenshots and renderer checks
- Finished: 2026-11-17T06:40:47Z
- Screenshots: https://github.com/aevalmere/aevaleron/tree/shots/rust-d-ui/rust/ask-do/19cd3887bacf84022712f7f1ce7b82c298a464b4 (/ in the ref becomes -)

| Step | Outcome | Log |
|---|---|---|
| Checkout | success |  |
| Node | success |  |
| Bridge unit checks | success | [bridge.log](bridge.log) |
| Install Playwright | success | [playwright.log](playwright.log) |
| Screenshots and renderer checks | failure | [shots.log](shots.log) |
| Upload screenshots | success |  |
| Publish to the shots branch | success |  |

errors.txt: 4 error and warning lines from the logs.
