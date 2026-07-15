# Premier party metadata findings

## Current evidence

Pinned demoparser's public 60.6 MB `test_demo.dem` fixture contains one message 350 at tick 56705. It has a reservation with 10 nonzero account IDs, 10 party IDs, no teammate colors, and no game type. The redacted party pattern is:

```text
P1 P1 P1 0 0 0 0 P1 P1 P1
```

The fixture has no documented Premier mode or party-composition ground truth. Six positions share one nonzero value, so this observation cannot establish that repeated nonzero values mean a player party. It does establish that message 350 is decodable through the pinned parser and that array cardinality can be inspected without exposing identifiers.

Three current replays downloaded through the local CS2 client were probed after the public fixture. Message 350 is absent from all three. Their companion `.dem.info` records contain 10 account IDs but omit game type, party IDs, teammate colors, and ranking types, so those records cannot independently establish Premier mode or queue composition. A fourth companion record was present without a completed Demo after the interrupted download and was not counted.

## Pending evidence

| Fixture | Mode | Queue ground truth | Message count | Account length | Party pattern | Color length | Verdict |
| --- | --- | --- | ---: | ---: | --- | ---: | --- |
| pinned public fixture | unknown | unknown | 1 | 10 | `P1 P1 P1 0 0 0 0 P1 P1 P1` | 0 | semantics unknown |
| local current replay A | pending confirmation | pending confirmation | 0 | not applicable | not applicable | not applicable | message absent |
| local current replay B | pending confirmation | pending confirmation | 0 | not applicable | not applicable | not applicable | message absent |
| local current replay C | pending confirmation | pending confirmation | 0 | not applicable | not applicable | not applicable | message absent |

## Provisional product verdict

No-go for v1 party labels. If Noah confirms that the local replays cover current Premier play, their shared absence is direct negative evidence against Demo-resident party metadata, regardless of queue composition. Treat absent messages, malformed messages, missing reservations, zero values, singleton nonzero values, array-length mismatches, and unvalidated repeated values as unknown. The optional metadata decoder must not make an otherwise valid Demo import fail.
