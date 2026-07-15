# Premier party metadata findings

## Current evidence

Pinned demoparser's public 60.6 MB `test_demo.dem` fixture contains one message 350 at tick 56705. It has a reservation with 10 nonzero account IDs, 10 party IDs, no teammate colors, and no game type. The redacted party pattern is:

```text
P1 P1 P1 0 0 0 0 P1 P1 P1
```

The fixture has no documented Premier mode or party-composition ground truth. Six positions share one nonzero value, so this observation cannot establish that repeated nonzero values mean a player party. It does establish that message 350 is decodable through the pinned parser and that array cardinality can be inspected without exposing identifiers.

## Pending evidence

| Fixture | Mode | Queue ground truth | Message count | Account length | Party pattern | Color length | Verdict |
| --- | --- | --- | ---: | ---: | --- | ---: | --- |
| pinned public fixture | unknown | unknown | 1 | 10 | `P1 P1 P1 0 0 0 0 P1 P1 P1` | 0 | semantics unknown |
| current Premier known party | Premier | pending | pending | pending | pending | pending | pending |
| current Premier solo queue | Premier | solo | pending | pending | pending | pending | pending |

## Provisional product verdict

No-go for v1 party labels until the pending Premier fixtures are tested. Treat absent messages, malformed messages, missing reservations, zero values, singleton nonzero values, array-length mismatches, and unvalidated repeated values as unknown. The optional metadata decoder must not make an otherwise valid Demo import fail.
