# Steam demo acquisition reference

Status: research for [Steam demo acquisition: share codes, the Game Coordinator,
and QR sign-in](https://github.com/NtrpyDev/openfrag/issues/5). Terminology
follows `CONTEXT.md`: a **Share Code** is the `CSGO-xxxxx` token identifying a
Valve matchmaking Match, and a **Demo** is Valve's `.dem` recording.

## Outcome

There are two technically plausible architectures. This research does not choose between them, and neither is release-ready:

1. **Installed Steamworks runtime.** Use the installed Steamworks runtime and `ISteamGameCoordinator` for app 730 while Steam is logged in, as community tools do. openfrag stores no Steam authentication token. There is no documented API for borrowing the installed Steam client's CM/GC session or credential, so this must not be described as supported session borrowing.
2. **Owned CM connection.** openfrag implements a Steam Connection Manager (CM) client, authenticates by QR/mobile confirmation, starts an app 730 GC session, and persists the resulting refresh token. This supports an independent daemon session but introduces secret storage and substantial unproven Rust protocol work.

The product choice must graduate to a separate decision/prototype ticket. It also has a policy gate: Valve authorization and legal review are required before automatic CM/GC acquisition can ship. A password form is forbidden for both architectures. Manual Share Code paste is only an alternate input because resolving it still automates a GC request. Only manual Demo import is independent of Steam automation.

## Release policy gate

Valve's current Steam Subscriber Agreement section 4.C prohibits scripts, bots, macros, and other non-human-controlled systems from interacting with Steam Content and Services "in any manner." Section 2.G separately restricts emulation or redirection of Valve communication protocols without prior written consent ([Steam Subscriber Agreement](https://store.steampowered.com/subscriber_agreement/english/)). Architecture B directly emulates Steam network protocol behavior, and both architectures automate GC interaction. Product conclusion: neither automatic architecture is release-ready without explicit Valve authorization and legal review. This is a release blocker, not merely a technical risk.

## Evidence classes

- **Official:** Valve's generic Steamworks and Web API documentation. Valve does not publish the CS2-specific match-history protocol as a supported API.
- **Community source-code evidence:** pinned implementations and mirrored protobufs that demonstrate observed protocol behavior. These can become stale when Valve changes CS2.
- **Product inference:** an openfrag design proposed from the evidence. It is not a claim about Valve's contract.

## What the official GC interface guarantees

Valve's generic `ISteamGameCoordinator` interface exposes `SendMessage`, `IsMessageAvailable`, and `RetrieveMessage`, and returns `EGCResult` values for delivery and retrieval ([Valve `ISteamGameCoordinator`](https://partner.steamgames.com/doc/api/isteamgamecoordinator?language=english)). It does not document CS2 request IDs, protobuf schemas, recent-Match limits, or Demo URL lifetime.

## Community-observed CS2 GC flow

The following details are community source-code evidence, not a Valve-supported contract:

1. Reach an app 730 GC session and wait until it is ready.
2. To obtain recent Matches, send `CMsgGCCStrike15_v2_MatchListRequestRecentUserGames` with the account ID. Its observed message ID is **9141**.
3. To resolve one Share Code, decode it into `matchId`, `outcomeId`, and `tokenId`, then send `CMsgGCCStrike15_v2_MatchListRequestFullGameInfo`. Its observed message ID is **9147**.
4. Both requests are handled as `CMsgGCCStrike15_v2_MatchList`, observed response ID **9139**.
5. Read the Demo URL from the `map` field of the last `roundstats` entry in the returned Match data, then download it promptly.

The pinned Boiler Writter implementation constructs the recent request with the current account ID, constructs the full-info request with `matchid`, `outcomeid`, and `token`, and registers the same Match-list response handler for both ([`CSGOMatchList.cpp`](https://github.com/akiver/boiler-writter/blob/fafb37c1b9bc53e7adff9c4f9ab3b5da68064da0/boiler-writter/CSGOMatchList.cpp)). Its GC client sends and receives protobuf messages through Steamworks and dispatches them by message type ([`CSGOClient.cpp`](https://github.com/akiver/boiler-writter/blob/fafb37c1b9bc53e7adff9c4f9ab3b5da68064da0/boiler-writter/CSGOClient.cpp)). The pinned community protobuf mirror defines these request and response message types and the Match data containing repeated `roundstats` ([`cstrike15_gcmessages.proto`](https://github.com/SteamDatabase/Protobufs/blob/a8658d7a579eeb9feed0cd20ff0295e3414c3f5b/csgo/cstrike15_gcmessages.proto)). The pinned Share Code implementation decodes the base-57 code into the three integers required by the full-info request ([`csgo-sharecode`](https://github.com/akiver/csgo-sharecode/blob/753f16fe97f9bbb121fb56675b40f035ad403d05/src/index.ts)).

This flow must be integration-tested against current CS2 before implementation is considered viable.

## Web API cursor, not Demo resolution

`ICSGOPlayers_730/GetNextMatchSharingCode` advances from a previously known Match authentication code and returns another Share Code. It does not return a Demo URL and does not replace the GC full-match-info request. The available description is on the Valve Developer Community wiki, which is community-maintained despite being hosted on a Valve domain, so treat it as historical implementation guidance rather than official Steamworks API documentation ([Access Match History](https://developer.valvesoftware.com/wiki/Counter-Strike:_Global_Offensive_Access_Match_History)).

Product inference: openfrag does not need to ask the user for a Steam Web API key merely to resolve a Share Code. If cursor-based history discovery is later desired, it should be evaluated separately from Demo URL acquisition.

## Architecture A: installed Steamworks runtime

### Flow

1. Require Steam to be running and the intended account to be logged in.
2. Initialize the Steamworks client context for app 730 and obtain `ISteamGameCoordinator`.
3. Establish GC readiness, then run the observed request/response flow above.
4. Hand each resolved URL to the common downloader.

### Consequences

- openfrag stores **zero Steam authentication tokens**. Steam owns login, mobile confirmation, credential lifecycle, and account switching.
- The active Steam account is the account whose Matches openfrag sees.
- openfrag depends on local Steam and possibly CS2 process state.
- Valve's Steamworks authentication documentation covers authentication
  tickets and OpenID, not borrowing a user's Steam client credential or CM/GC
  session. No documented session-borrowing or token-borrowing API was found
  ([Valve user authentication and ownership](https://partner.steamgames.com/doc/features/auth)).
- It is unknown whether a Rust daemon can initialize the required app 730 context reliably on Linux without launching CS2, whether it can coexist with a running CS2 instance, and what redistribution constraints apply to the Steamworks SDK.

This architecture is demonstrated conceptually by Boiler Writter's Steamworks-based GC client, but not yet by an openfrag Linux/Rust proof of concept ([`CSGOClient.cpp`](https://github.com/akiver/boiler-writter/blob/fafb37c1b9bc53e7adff9c4f9ab3b5da68064da0/boiler-writter/CSGOClient.cpp)).

## Architecture B: owned CM connection with QR login

### Flow

1. Start Steam's QR authentication session and display the one-time QR locally.
2. Poll until the user confirms in the Steam mobile app.
3. Establish and maintain a CM connection, persist the refresh token, and start app 730 as played.
4. Reach GC readiness and run the same observed CS2 request/response flow.
5. On disconnect, reconnect with bounded backoff. A local disconnect-and-forget action deletes openfrag's token, but server-side revocation is not proven.

Valve's Steam Guard FAQ confirms that a QR code can be scanned in the Steam mobile app and approved without entering the account password into the requesting device ([Steam Guard Mobile Authenticator FAQ](https://help.steampowered.com/en/faqs/view/7EFD-3CAE-64D3-1C31)). The actual polling, access-token, and refresh-token protocol is not an official public API. SteamRE's open-source DepotDownloader and pinned SteamKit authentication sample are community implementation evidence for that machinery ([DepotDownloader README](https://github.com/SteamRE/DepotDownloader), [SteamKit authentication sample](https://github.com/SteamRE/SteamKit/blob/ccdffb73866445deddb50f0a04117e60d585a368/Samples/001_AuthenticationWithQrCode/Program.cs#L37-L65)).

### Token storage

Product inference if Architecture B is chosen: persist only the refresh token in the current user's Linux Secret Service collection, never plaintext, SQLite, configuration files, or logs. Keep the access token and QR payload in memory only. Store the Steam account ID and non-secret acquisition state in SQLite. Secret Service item attributes are not secret, and the keyring does not protect against root or same-user malware while unlocked ([Secret Service specification](https://specifications.freedesktop.org/secret-service/latest-single/)). A disconnect-and-forget action deletes local secrets but must not claim server revocation. Exact token rotation, revocation, expiry, and concurrent-session behavior must be established by prototype.

### Rust position

`steam-client-rs` exposes a Rust Steam client and GC transport ([crate documentation](https://docs.rs/steam-client-rs)); its GC module handles protobuf-framed per-app messages ([GC source](https://docs.rs/steam-client-rs/latest/src/steam_client/services/gc.rs.html)). It does not establish that the complete current CS2 QR login, app 730 session, protobuf job correlation, and Demo flow works end to end. No verified maintained Rust crate was found that provides that complete high-level path. Architecture B therefore carries meaningful protocol, maintenance, and security work.

## Availability window

CS Demo Manager reports that the GC exposes the last eight Valve matchmaking Matches and that Demo download links stop working about one month after the Match ([CS Demo Manager download guide](https://cs-demo-manager.com/docs/guides/downloads)). These are community observations, not Valve guarantees. Do not describe them as a fixed API limit, a signed-URL time-to-live, or an exact expiry timestamp.

Product inference: poll after a Match, resolve its URL, and download immediately. A missing recent Match can be retried for a short bounded period. An old Share Code whose full-info response has no usable URL, or a URL that consistently returns an unavailable response, becomes a terminal `expired_or_unavailable` state with guidance to import a local Demo.

## Common download state machine

The authentication and GC implementation ends at `ResolvedDemo { match_identity, source_url }`. A shared downloader should then:

1. Stream into a uniquely named temporary file in the target filesystem. Never buffer the whole Demo in memory.
2. Require a successful HTTP status and reject redirects or hosts outside the explicitly accepted Valve/CDN policy determined by the prototype.
3. Apply a compressed-size limit and request timeout.
4. Decompress if required, then verify the Demo header begins with `PBDEMS2` before accepting it.
5. Hash the verified Demo, deduplicate by content hash and Match identity, fsync as appropriate, and atomically rename it into the Demo store.
6. Retry transport failures and 5xx responses with bounded exponential backoff. Treat permanent HTTP errors, invalid content, and exhausted retries as explicit failures. Treat confirmed expiry or unavailability as terminal.

HTTP headers such as `Content-Length` are validation aids, not proof of a valid Demo. The header check, hashing, deduplication, and atomic rename are openfrag product requirements inferred for crash safety and hostile-input handling.

## Manual paths

### Share Code paste

Accept `CSGO-...`, trim surrounding whitespace, decode and validate it locally, then use the selected architecture's GC full-info request. A valid Share Code carries `matchId`, `outcomeId`, and `tokenId`; it does not itself contain the Demo URL ([pinned decoder](https://github.com/akiver/csgo-sharecode/blob/753f16fe97f9bbb121fb56675b40f035ad403d05/src/index.ts)). Show `expired_or_unavailable` rather than retrying indefinitely. This is not an independent fallback because URL resolution still performs automated GC interaction and remains behind the release policy gate.

### Manual Demo import

Accept a local `.dem` file without Steam login, GC access, or a Share Code. Copy through the same temporary-file, `PBDEMS2`, hash, deduplication, and atomic-rename pipeline, then enqueue parsing. Compressed manual imports should remain unsupported until accepted compression formats and bomb limits are specified and tested.

Until the release policy gate is cleared, manual Demo import is the only acquisition state allowed in a release build. Automatic discovery and Share Code resolution remain disabled research/prototype states.

## Module seam

Keep unstable Steam protocol code behind one narrow acquisition boundary:

```text
MatchDiscovery -> ShareCode
DemoResolver.resolve(ShareCode) -> ResolvedDemo
DemoDownloader.fetch(ResolvedDemo) -> StoredDemo
DemoImporter.import(path) -> StoredDemo
```

`DemoResolver` has two prototype implementations: `InstalledSteamworksGcResolver` and `CmQrGcResolver`. Both remain disabled in release builds until the `valve_authorization_and_legal_review` gate is cleared. Both consume the same generated CS2 protobuf types and return the same error vocabulary: `policy_blocked`, `steam_unavailable`, `auth_required`, `gc_unavailable`, `invalid_share_code`, `expired_or_unavailable`, `protocol_changed`, and `temporary_failure`. The downloader and importer do not know which Steam architecture was selected. `DemoImporter` bypasses that gate because it does not interact with Steam.

## Prototype and decision unknowns

- Can Architecture A initialize app 730 and receive GC response 9139 reliably on the target Linux distributions, both while CS2 is closed and while it is running?
- Does Architecture A require shipping Valve SDK artifacts, and are their license and packaging terms compatible with openfrag?
- Can the candidate Rust stack complete QR confirmation, refresh-token reuse, CM reconnect, app-730 presence, and GC job correlation without filling major protocol gaps?
- Are message IDs 9141, 9147, and 9139 and the `roundstats.last().map` location still current for CS2?
- What exact URL hosts, redirect behavior, compression format, maximum size, and HTTP failure responses occur in current regions?
- How do account switching, concurrent Steam sessions, mobile revocation, and offline startup behave?
- How quickly does a completed Match appear, how stable is the observed eight-Match window, and how does about-one-month availability vary?
- Will Valve authorize either automated GC architecture for openfrag, and what legal or product constraints would that authorization impose?

The prototype should exercise one real Match and one manual Share Code through both architectures where feasible, capture protocol failures without secrets, and produce a written product decision. Until then, neither architecture is the committed openfrag design.
