# Clip review workflow v1

## Decision

The dashboard presents every saved Clip in a review queue, then lets the
player keep, edit metadata, or delete it. A Clip is linked to its Highlight,
Capture Session, Match when known, and source evidence state. Auto Highlights
are provisional until the Demo confirms their round evidence. Manual Flags are
provisional unless a matching Capture Session and saved replay are present.
Demo confirmation never changes the video file silently. It updates evidence
and labels on the existing Clip.

## Review flow

1. **Inbox.** Show newest Clips first with thumbnail, duration, source
   (Auto Highlight or Manual Flag), evidence badge (Provisional or Demo
   confirmed), Capture Session, Match and round when known, and storage size.
2. **Player.** Open a local video player with scrubbing, play or pause, and a
   single trim range. Keep the original file until the edited result is
   successfully written and verified.
3. **Edit.** Save a trimmed derivative, editable title, and favorite state.
   The title defaults to the Highlight type and timestamp. Names are ordinary
   local text, not inferred claims about skill.
4. **Keep or delete.** Keep removes the Inbox state. Delete requires a
   confirmation showing the exact file and removes the derivative and metadata
   only after the file operation succeeds. The original is not recoverable in
   v1 after confirmed deletion.
5. **Export.** Export a selected Clip to a user-chosen ordinary file. The
   Discord option creates an MP4 derivative using H.264 video and AAC audio,
   targeting a configurable size ceiling, then reports the actual size. It
   never uploads or contacts Discord.

## Merged triggers

Multiple triggers in one Capture Session and overlapping time windows produce
one Clip with one canonical Highlight record and a list of contributing
triggers. The review card shows the combined label, for example “multikill +
Manual Flag”, and links each trigger receipt. A later trigger that falls
outside the existing window creates a separate Clip. Merging is deterministic
and does not duplicate video files.

## V1 boundary

V1 includes local queue review, playback, one trim range, rename, favorite,
delete confirmation, deterministic merged-trigger presentation, and explicit
Discord-friendly export to a local file. It stores Capture Session identity,
source, evidence status, Match and round links, parser and formula versions
when available, and immutable file provenance.

Later work includes multi-segment editing, captions, automatic reframing,
batch export, upload or share links, cloud backup, collaborative review,
undo/recycle-bin recovery, and per-platform encoding presets beyond the one
Discord-friendly option.

## Five-round choices

### Queue versus immediate playback

The queue wins because Auto Highlights save at round end and Manual Flags can
arrive in bursts. A queue makes provisional state and failure recovery visible.

### Destructive trim versus derivative trim

Derivative trim wins because it preserves the original evidence-linked file and
allows a failed encode to be retried. V1 exposes one trimmed derivative only.

### Automatic naming versus free naming

Free naming with a conservative default wins. Automatic names are useful for
finding files, but inferred labels must not masquerade as verified facts.

### Hard delete versus recycle bin

Confirmed hard delete wins for v1 simplicity and privacy, provided the exact
path and metadata are shown before confirmation. Recovery belongs to later
work.

### Upload versus local export

Local export wins. The product promises no upload path, so Discord support means
producing a compatible file for the player to attach manually.

## State and failure rules

Clip states are `saved`, `in_review`, `kept`, `edited`, `favorite`, and
`deleted`; favorite is a flag, not an exclusive state. Failed encoding,
missing source, or failed deletion remains visible with an actionable error and
does not silently change state. A Clip whose Capture Session ended before a
valid replay save is marked incomplete and cannot be presented as confirmed.

The dashboard distinguishes `Provisional` from `Demo confirmed` in every card
and detail view. If the Demo is unavailable, the Clip remains playable and
provisional. If parsing later fails, preserve the provisional Clip and its
failure Receipt rather than deleting or inventing confirmation.

## Unresolved dependencies

- The resolved highlight contract must provide exact merge-window and
  post-roll values.
- The Capture Session contract must define its stable identifier and lifecycle.
- gpu-screen-recorder output and codec availability determine whether the
  Discord derivative can meet its size ceiling.
- Demo confirmation must provide stable Match and round identifiers.
