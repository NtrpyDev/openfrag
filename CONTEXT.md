# openfrag glossary

The ubiquitous language for this project. Terms mean exactly this everywhere: code, issues, docs, UI copy.

- **GSI**: Game State Integration, Valve's official mechanism where CS2 POSTs live JSON state snapshots to a local HTTP endpoint. openfrag's only live data source. VAC-safe by design.
- **Highlight**: a moment worth keeping, detected either automatically from GSI events or flagged manually by the player.
- **Auto Highlight**: a highlight finalized from Demo evidence. GSI can raise a provisional candidate, but clutch and knife labels require the Demo.
- **Manual Flag**: the player pressing the global hotkey to say "clip that". Saves the replay buffer immediately, not at round end.
- **Clip**: the video file produced for a highlight, cut from the replay buffer.
- **Replay Buffer**: gpu-screen-recorder's rolling 60-second recording used for provisional round captures and Manual Flags.
- **Demo**: Valve's `.dem` recording of a full match, imported from a local file in v1. The source for all canonical post-match analytics.
- **Share Code**: the CSGO-xxxxx token identifying a Valve matchmaking match, used to locate its demo.
- **Match**: one Premier game, with its demo, parsed stats, and any clips linked to its rounds.
- **Lobby**: all ten players in a match. Every one of them is browsable in the dashboard.
- **openfrag Rating**: the project's explainable per-match performance score. Its formula is public and every component links to the rounds that produced it.
- **Receipt**: the evidence link from a stat to its rounds or clips. Clicking "lost 4 opening duels" opens those four rounds.
- **Tonight**: the dashboard home view, the current play session's matches, stats, and best clips.
- **Setup Wizard**: the first-run flow in the dashboard that configures storage, writes the GSI config into CS2, and tests recording, GSI, and the Manual Flag path.
- **Compatibility Doctor**: the wizard's diagnostic step, verifying compositor, encoder, hotkey, GSI, and Steam location before first use. Re-runnable from settings.
- **Release Candidate**: one exact versioned source commit and the proposed Channel Artifacts derived from it. Any source, recipe, dependency, or artifact change creates a different candidate.
- **Channel Artifact**: the installable output for one distribution channel: a GitHub static archive, an AUR package built from one exact pkgbase commit, or a COPR RPM from one exact build. Artifacts from different channels never inherit each other's qualification.
- **Qualification Record**: the redacted support-matrix evidence tying one Channel Artifact to its exact build inputs, hash, host fingerprint, observed gates, failures, and limitations.
