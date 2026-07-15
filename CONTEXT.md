# openfrag glossary

The ubiquitous language for this project. Terms mean exactly this everywhere: code, issues, docs, UI copy.

- **GSI**: Game State Integration, Valve's official mechanism where CS2 POSTs buffered live state snapshots to a local HTTP endpoint. It is openfrag's only live match-state source, not an event log. VAC-safe by design.
- **Highlight**: a moment worth keeping, detected either automatically from GSI events or flagged manually by the player.
- **Auto Highlight**: a highlight detected by rules (multikill, clutch win, knife kill). Its clip is saved at round end so the whole play is captured.
- **Manual Flag**: the player pressing the global hotkey to say "clip that". Saves the replay buffer immediately, not at round end.
- **Clip**: the video file produced for a highlight, cut from the replay buffer.
- **Replay Buffer**: gpu-screen-recorder's rolling in-memory recording of the last N seconds of gameplay.
- **Demo**: Valve's `.dem` recording of a full match, selected from local disk in v1. The authoritative source for post-match analytics.
- **Share Code**: the CSGO-xxxxx token identifying a Valve matchmaking match, used to locate its demo.
- **Match**: one Premier game, with its demo, parsed stats, and any clips linked to its rounds.
- **Lobby**: all ten players in a match. Every one of them is browsable in the dashboard.
- **Local Player**: the player whose SteamID64 the user explicitly confirms from authenticated GSI evidence or an imported Demo roster. Personal Rating and trends belong only to this player.
- **openfrag Rating**: the project's explainable per-match performance score. Its formula is public and every component links to the rounds that produced it.
- **Receipt**: the evidence link from a stat to its rounds or clips. Clicking "lost 4 opening duels" opens those four rounds.
- **Tonight**: the dashboard home view, the current play session's matches, stats, and best clips.
- **Setup Wizard**: the resumable first-run dashboard flow that configures independent local capabilities and allows unavailable optional capabilities to be revisited from Settings.
- **Compatibility Doctor**: the shared read-only diagnostic model behind first run and Settings, with explicit consent required for every repair or active test.
