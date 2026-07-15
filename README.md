# openfrag

Your best CS2 moments and your local match history: captured, analyzed, and kept on your Linux PC.

openfrag is one local app. It watches your game through Valve's official Game State Integration and saves provisional round captures while you play. After you import a local Premier demo, it confirms and labels 3k, 4k, ace, clutch, and knife highlights and turns the match into a stats dashboard served from your own machine.

## What it does

- **Clips itself.** Provisional round captures use a 60-second replay buffer, then Demo evidence confirms 3k, 4k, ace, clutch wins, and knife kills. One hotkey immediately saves the preceding play for moments the rules cannot measure.
- **Stats with receipts.** An explainable rating trend with the formula in the open, never a mystery score. Opening duels, clutch conversions, utility effectiveness, death context. Every number links to the rounds that produced it.
- **Opens on the answer.** How did I do tonight, am I getting better, and your best moments of the session, playable right on the page.
- **Private by architecture.** No openfrag account, no backend, no telemetry, no upload path, and no Steam sign-in. V1 imports local `.dem` files selected by the player.

## Status

V1 implementation is in progress from the decisions on the [wayfinder map](../../issues/1).

## Support status

No environment is release-qualified yet. Physical and read-only prototypes have established limited KDE and GNOME Wayland evidence, while installed-package and recorder gates remain open. See the exact [support matrix](docs/support-matrix.md).

## Stack

Rust daemon plus a local web dashboard. Recording via [gpu-screen-recorder](https://git.dec05eba.com/gpu-screen-recorder/about/) (run as a separate process). Single-file SQLite metadata; demos and clips stored as ordinary files.

## Verification

Run `./scripts/verify-ci.sh fast` for the optimized developer subset or `./scripts/verify-ci.sh` for the complete v1 suite. The [whole-app verification guide](docs/testing.md) documents coverage, concurrency, clean package environments, timing budgets, and retained failure logs.

## License

GPL-3.0. The code you install stays open and auditable.
