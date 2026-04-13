# Live Execution Timeline

## What This Feature Does

The Live Execution Timeline is a TUI panel that shows each assistant run as a sequence of visible steps.

It makes execution understandable in real time by showing:

- tool activity (start, finish, error, cancelled)
- turn summaries
- status and error notes
- per-row duration, token deltas, and cost deltas

## Why It Exists

Without a timeline, long runs feel opaque.

This panel improves:

- observability: users can see what is happening now
- debugging speed: users can spot where a run stalled or failed
- trust: users can inspect execution details instead of waiting blindly

## How To Use

Start the app:

```powershell
cd src-rust
cargo run --package claurst
```

Open timeline:

- `/timeline show`
- or `Ctrl+Shift+L` (toggle)

Hide timeline:

- `/timeline hide`
- or `Ctrl+Shift+L`

Clear retained timeline rows:

- `/timeline clear`

## Timeline Navigation

When timeline has focus:

- `Up` / `Down` or `j` / `k`: move selection
- `Enter`, `Right`, or `Space`: expand/collapse selected row
- `Left`: collapse selected row
- `Esc`: move focus back to input

To focus timeline again after `Esc`:

- `/timeline show`
- or toggle with `Ctrl+Shift+L`

## What You Should See

Example rows:

- `✓ [tool] Reading file: README.md`
- `✓ [tool] Running command: cargo fmt --all`
- `! [note] Error: failed to parse config`
- `✓ [turn] Assistant turn 2 finished`

Expanded rows show:

- preview text
- detailed content (input/result/status/error details)

## Layout Behavior

- Wide terminal: timeline appears on the right.
- Medium terminal: timeline appears at the bottom.
- Small terminal: compact details rendering is used so expanded rows still show readable text.

## Known Limits

- Full compile/test verification may fail on systems missing a C toolchain (`gcc.exe`) due to `libsqlite3-sys`.
- On very narrow terminal sizes, labels/details are truncated for readability.

