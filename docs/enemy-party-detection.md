# Enemy party detection in Premier data

## Decision

Yes, the Counter-Strike 2 protocol defines a direct path for party data to appear in a demo packet. `CS_UM_ServerRankRevealAll` embeds a matchmaking reservation containing `account_ids`, `party_ids`, and `teammate_colors`. Presence is not guaranteed, and the current pinned demoparser generates the relevant protobuf types but drops this message during packet dispatch.

Openfrag may expose party grouping only when a representative Premier fixture confirms the message is present and establishes the array and zero-value semantics. Otherwise party membership remains unknown. Openfrag must never infer a party from teammate color, clan text, friendship, or behavior.

Confidence is high that the protocol and pinned parser contain the direct data path. Confidence is low about current Premier emission and field semantics because no representative fixture was available locally.

## What the demo contains

The inspected CS2 demo schema defines headers, packets, full packets, string tables, console commands, and opaque custom data. Its header and player-info structures do not directly declare a party relation ([demo schema at GameTracking commit `fe4e895`](https://github.com/SteamDatabase/GameTracking-CS2/blob/fe4e895b3b44d4c1e4ae32e4b62daa6150d6c8f0/Protobufs/demo.proto)).

The player-info message contains a name, XUID, user ID, Steam ID, and bot or broadcast flags. It has no party identifier ([network base types at the same commit](https://github.com/SteamDatabase/GameTracking-CS2/blob/fe4e895b3b44d4c1e4ae32e4b62daa6150d6c8f0/Protobufs/networkbasetypes.proto)). The schema snapshot comes from SteamDatabase's extracted game files, so this conclusion is scoped to the inspected revision rather than an official promise that Valve will never add such data.

The pinned demoparser revision exposes player identity, team, clan text, competitive teammate color, entity properties, and events, but no party relation ([parser API](https://github.com/LaihoE/demoparser/blob/ba39cc44cd5abfd7f34df2b3c0a7dd3630048311/src/parser/src/parse_demo.rs), [supported fields](https://github.com/LaihoE/demoparser/blob/ba39cc44cd5abfd7f34df2b3c0a7dd3630048311/README.md), [field mappings](https://github.com/LaihoE/demoparser/blob/ba39cc44cd5abfd7f34df2b3c0a7dd3630048311/src/parser/src/maps.rs)). Its generated protobuf module includes `CcsUsrMsgServerRankRevealAll` and the reservation fields, but its packet dispatcher has no handler for message type 350 ([generated protobuf](https://github.com/LaihoE/demoparser/blob/ba39cc44cd5abfd7f34df2b3c0a7dd3630048311/src/csgoproto/src/protobuf.rs), [second-pass dispatcher](https://github.com/LaihoE/demoparser/blob/ba39cc44cd5abfd7f34df2b3c0a7dd3630048311/src/parser/src/second_pass/parser.rs)). Supporting the field requires a small parser extension plus fixture tests, not a heuristic.

## Direct reservation path

The extracted Game Coordinator schema defines a server reservation containing `account_ids`, `party_ids`, and `teammate_colors` ([Game Coordinator messages at GameTracking commit `fe4e895`](https://github.com/SteamDatabase/GameTracking-CS2/blob/fe4e895b3b44d4c1e4ae32e4b62daa6150d6c8f0/Protobufs/cstrike15_gcmessages.proto)). The user-message schema then embeds that reservation in `CCSUsrMsg_ServerRankRevealAll`, message type 350 ([CS2 user messages at the same commit](https://github.com/SteamDatabase/GameTracking-CS2/blob/fe4e895b3b44d4c1e4ae32e4b62daa6150d6c8f0/Protobufs/cstrike15_usermessages.proto)). Because demo packets carry network and user messages, this is a defined route for reservation-backed party data into a demo.

The schema does not document whether current Premier demos emit this message, whether the arrays align by index, what zero means, or whether singleton party IDs are meaningful. [Premier party metadata spike: validate ServerRankRevealAll](https://github.com/NtrpyDev/openfrag/issues/26) must settle those points before party metadata becomes a product input.

## Rejected inferences

None of these are reliable party evidence:

- competitive teammate color, which is presentation state;
- clan text or similar player names;
- team membership, join timing, coordination, purchases, chat, or voice activity;
- friendship graphs or repeated co-occurrence across matches.

Treating those signals as party membership would create false claims about players and could introduce unnecessary social-graph collection.

## Verification gap

The local development machine had no representative `.dem` sample and no installed demoparser executable during this investigation. The schema and parser findings are therefore source-level findings. The follow-up prototype should record only message presence, array cardinality, repeated nonzero group patterns, and redacted identifiers.

## Product contract

- Derive team and opponent relationships from observed demo team assignments.
- Decode `CS_UM_ServerRankRevealAll` and require a present reservation with fixture-validated array semantics.
- Group only players backed by the same validated, nonzero party identifier. Treat absent, zero, singleton, mismatched, or malformed values as unknown until fixtures prove their meaning.
- Show anonymous within-match labels such as `Party A`; never display or persist raw party IDs.
- Do not emit own-party or enemy-party labels from teammate color, clan text, friendship, repeated encounters, or behavioral heuristics.
