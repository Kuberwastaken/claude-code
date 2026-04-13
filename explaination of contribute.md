# Live Execution Timeline (Simple Explanation)

## What is this feature?

Live Execution Timeline is a panel in the terminal UI that shows what the agent is doing step by step.

Instead of waiting blindly, you can see:

- which tool is running
- which step finished
- where an error happened
- how long each step took
- token and cost changes per step

## Why is it useful?

- Easy to understand progress during long tasks
- Easier to debug failures
- Easier to trust agent behavior

## How to open and use it

Open timeline:

- `/timeline show`
- or `Ctrl+Shift+L` (toggle)

Hide timeline:

- `/timeline hide`

Clear old rows:

- `/timeline clear`

Navigate inside timeline:

- `Up` / `Down` or `j` / `k` to move
- `Enter` (or `Right` / `Space`) to expand selected row
- `Left` to collapse
- `Esc` to move focus back to input

If focus is lost after `Esc`, use `/timeline show` again.

## Example (normal run)

You ask:

`"Read README and summarize changes in src-rust/crates/tui"`

Timeline can show:

```text
[done] [tool] Reading file: README.md
[done] [tool] Searching code: timeline
[done] [tool] Running command: cargo fmt --all
[done] [turn] Assistant turn 2 finished
```

If you press `Enter` on a row, details appear, for example:

```text
Reading file: README.md | Done | 435ms
Preview: README.md
Details: Read first 200 lines and extracted key sections.
```

## Example (error case)

Timeline can show:

```text
! [note] Error: failed to parse config
```

Expanded details show the error text so you can quickly see what failed.

## Quick notes

- Wide terminal: timeline appears on the right.
- Medium terminal: timeline appears at the bottom.
- Small terminal: compact details mode is used.

