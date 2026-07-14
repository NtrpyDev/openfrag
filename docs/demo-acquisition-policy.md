# Demo acquisition policy for v1

## Decision

v1 ships local `.dem` file import only. Automatic recent-match discovery, Share Code resolution, Game Coordinator access, Steam sign-in, and download-URL retrieval are disabled.

This is a product and risk decision, not legal advice. The factual basis is that Valve documents a public Steamworks Web API, but its official interface list does not document a CS2 Premier demo or Share Code resolver ([Web API reference](https://partner.steamgames.com/doc/webapi), [Web API overview](https://partner.steamgames.com/doc/webapi_overview)). Issue 5 research also found no verified end-to-end Rust implementation for the required Game Coordinator flow. Valve's current Subscriber Agreement restricts unauthorized automation and tampering and prohibits emulating or redirecting Valve network protocols without prior written consent ([official agreement](https://store.steampowered.com/subscriber_agreement/)). Those terms and documentation inform the boundary; they are not a legal interpretation.

## Allowed v1 behavior

- The Setup Wizard lets a user choose an existing `.dem` file from local disk.
- openfrag validates that the selected file is readable, copies it into local match storage, hashes it, deduplicates it, and parses it locally.
- The product may show import progress, parse errors, demo metadata, and the resulting stats and Receipts.
- No Steam credential, QR confirmation, Share Code, or remote match-history access is required for import.

## Prohibited v1 behavior

Do not collect or resolve Share Codes, sign into Steam, emulate Game Coordinator messages, discover recent matches automatically, fetch demo URLs, or imply that any of those paths are available. A pasted Share Code must receive a clear unavailable message and a manual-download/import instruction. Do not silently retry an unsupported automatic path or convert its failure into an empty Match.

## Setup Wizard and product copy

Use this promise: **“Import a demo file from disk. openfrag analyzes it locally; no Steam credentials or network match-history access are required.”**

The Wizard should explain that automatic Share Code and recent-match import are not in v1. It should offer a file picker, validate the path and hash, and report whether the import is complete, duplicate, corrupt, or unsupported. It must never present a Steam login screen for demo acquisition.

## Reopening automatic acquisition

Automatic acquisition can be reconsidered only after all gates pass:

1. Written Valve authorization or a documented supported API that covers the exact CS2 flow.
2. Legal and security review of authentication, token storage, protocol use, and data handling.
3. A maintained implementation with pinned protocol/schema fixtures, expiry and retry behavior, and explicit failure states.
4. An opt-in product decision with revised copy, privacy review, and a migration path that preserves local import.

Until then, local import is the complete and supported v1 acquisition path.

## Verification checklist

- Import a valid local Premier `.dem` and verify hash, deduplication, parsing, stats, and Receipts.
- Import the same file twice and verify one Match with an explicit duplicate result.
- Try a missing, unreadable, truncated, and non-demo file and verify a visible error, never an empty Match.
- Confirm the Wizard contains no Steam credential or QR flow and no automatic network acquisition path.
- Paste a Share Code and verify the unavailable message plus manual-import guidance.
- Inspect outbound connections during import and verify no Steam or remote demo-download request is required.
- Confirm the product still works when Steam is not running.
