# Premier party metadata prototype

## Question

Does a current Valve Premier Demo carry `CS_UM_ServerRankRevealAll` message 350 with a reservation whose `account_ids`, `party_ids`, and `teammate_colors` arrays can support exact party labels? The probe must distinguish absent, malformed, missing-reservation, mismatched-array, zero, singleton, and repeated nonzero cases without printing raw account or party identifiers.

## Run

Pass one or more decompressed Demo files to the probe:

```sh
./prototypes/premier-party-metadata/run.sh /path/to/known-party.dem /path/to/known-solo.dem
```

With no paths, the command downloads the pinned parser and runs its public `test_demo.dem` fixture. Set `DEMOPARSER_ROOT` to reuse a clean checkout. The runner verifies parser commit `ba39cc44cd5abfd7f34df2b3c0a7dd3630048311`, applies the narrow throwaway patch, and runs the decoder in single-threaded mode.

Premier downloads may arrive as `.dem.bz2`. Decompress them before running the probe. Do not add private Demo files to Git, and do not publish raw account IDs, party IDs, Share Codes, or match IDs.

## Output

Each Demo reports whether message 350 is absent or present. Present reservations show only:

- array lengths;
- positions containing zero account IDs;
- party equality as first-seen labels such as `P1`, `P2`, and `0`;
- teammate-color values;
- optional game type and shutdown metadata.

The labels reveal equality within one message but cannot be compared across messages or Demos. A repeated label is not accepted as party evidence until known-party and known-solo Premier fixtures establish its meaning.

## Fixture requirements

The decision needs multiple current Premier Demos, including at least one match with a known party and one known solo queue when available. Record only the uploader's ground-truth queue composition and the redacted probe output in `NOTES.md`. Also run at least one non-Premier fixture to document absence or incompatible semantics.
