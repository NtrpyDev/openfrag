# openfrag glossary

The ubiquitous language for this project. Terms mean exactly this everywhere: code, issues, docs, UI copy.

- **GSI**: Game State Integration, Valve's official mechanism where CS2 POSTs live JSON game events to a local HTTP endpoint. openfrag's only live data source. VAC-safe by design.
- **Highlight**: a moment worth keeping, detected either automatically from GSI events or flagged manually by the player.
- **Auto Highlight**: a highlight detected by rules (multikill, clutch win, knife kill). Its clip is saved at round end so the whole play is captured.
- **Manual Flag**: the player pressing the global hotkey to say "clip that". Saves the replay buffer immediately, not at round end.
- **Clip**: the video file produced for a highlight, cut from the replay buffer.
- **Replay Buffer**: gpu-screen-recorder's rolling in-memory recording of the last N seconds of gameplay.
- **Demo**: Valve's .dem recording of a full match, downloaded from Valve or imported manually. The source for all post-match analytics.
- **Share Code**: the CSGO-xxxxx token identifying a Valve matchmaking match, used to locate its demo.
- **Match**: one Premier game, with its demo, parsed stats, and any clips linked to its rounds.
- **Lobby**: all ten players in a match. Every one of them is browsable in the dashboard.
- **openfrag Rating**: the project's explainable per-match performance score. Its formula is public and every component links to the rounds that produced it.
- **Receipt**: the evidence link from a stat to its rounds or clips. Clicking "lost 4 opening duels" opens those four rounds.
- **Tonight**: the dashboard home view, the current play session's matches, stats, and best clips.
- **Setup Wizard**: the first-run flow in the dashboard that configures storage, writes the GSI config into CS2, tests recording and hotkey, and signs into Steam.
- **Compatibility Doctor**: the wizard's diagnostic step, verifying compositor, encoder, hotkey, GSI, and Steam location before first use. Re-runnable from settings.
