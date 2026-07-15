# openfrag

Your best CS2 moments and your local match history: captured, analyzed, and kept on your Linux PC.

openfrag is one local app. It watches your game through Valve's official Game State Integration and saves a clip every time you ace, clutch, or knife someone, automatically, with the whole play. After the match it imports your Premier demo and turns it into a stats dashboard served from your own machine.

## What it does

- **Clips itself.** 3k, 4k, ace, clutch wins, knife kills: captured without you touching anything. One hotkey saves the last 30 seconds for the plays the numbers can't measure.
- **Stats with receipts.** An explainable rating trend with the formula in the open, never a mystery score. Opening duels, clutch conversions, utility effectiveness, death context. Every number links to the rounds that produced it.
- **Opens on the answer.** How did I do tonight, am I getting better, and your best moments of the session, playable right on the page.
- **Private by architecture.** No openfrag account, no backend, no telemetry, no upload path. It connects to Steam only to fetch your demos, using Steam's QR/mobile-confirmation flow. openfrag never asks for your Steam password.

## Status

Pre-development. Planning happens in the open on the [wayfinder map](../../issues/1).

## Support status

No environment is release-qualified yet. Physical and read-only prototypes have established limited KDE and GNOME Wayland evidence, while installed-package and recorder gates remain open. See the exact [support matrix](docs/support-matrix.md).

## Stack

Rust daemon plus a local web dashboard. Recording via [gpu-screen-recorder](https://git.dec05eba.com/gpu-screen-recorder/about/) (run as a separate process). Single-file SQLite metadata; demos and clips stored as ordinary files.

## License

GPL-3.0. The code you install stays open and auditable.
