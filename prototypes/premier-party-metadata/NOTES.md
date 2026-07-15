# Premier party metadata findings

## Current evidence

Pinned demoparser's public 60.6 MB `test_demo.dem` fixture contains one message 350 at tick 56705. It has a reservation with 10 nonzero account IDs, 10 party IDs, no teammate colors, and no game type. The redacted party pattern is:

```text
P1 P1 P1 0 0 0 0 P1 P1 P1
```

The fixture has no documented Premier mode or party-composition ground truth. Six positions share one nonzero value, so this observation cannot establish that repeated nonzero values mean a player party. It does establish that message 350 is decodable through the pinned parser and that array cardinality can be inspected without exposing identifiers.

Three current Premier solo-queue replays downloaded through the local CS2 client were probed after the public fixture. Noah supplied the mode and queue-composition ground truth. Message 350 is absent from all three. Their companion `.dem.info` records contain 10 account IDs but omit game type, party IDs, teammate colors, and ranking types. A fourth companion record was present without a completed Demo after the interrupted download and was not counted.

## Pending evidence

| Fixture | Mode | Queue ground truth | Message count | Account length | Party pattern | Color length | Verdict |
| --- | --- | --- | ---: | ---: | --- | ---: | --- |
| pinned public fixture | unknown | unknown | 1 | 10 | `P1 P1 P1 0 0 0 0 P1 P1 P1` | 0 | semantics unknown |
| local current replay A | Premier | solo queue | 0 | not applicable | not applicable | not applicable | message absent |
| local current replay B | Premier | solo queue | 0 | not applicable | not applicable | not applicable | message absent |
| local current replay C | Premier | solo queue | 0 | not applicable | not applicable | not applicable | message absent |

## Provisional product verdict

No-go for v1 party labels. Three current Premier solo-queue Demos omit message 350, while the only fixture that contains it lacks mode and queue-composition ground truth. Current Premier emission is therefore not reliable, and the meaning of repeated nonzero party IDs remains unvalidated. Treat absent messages, malformed messages, missing reservations, zero values, singleton nonzero values, array-length mismatches, and unvalidated repeated values as unknown. The optional metadata decoder must not make an otherwise valid Demo import fail.
