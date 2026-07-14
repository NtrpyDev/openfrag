# Clip review workflow v1

## Decision

The dashboard presents every saved Clip in a review queue, then lets the
player keep, edit metadata, or delete it. A Clip is linked to its Highlight,
Capture Session, Match when known, and source evidence state. Auto Highlights
are provisional until the Demo confirms their round evidence. Manual Flags
represent independent user intent; their media save is complete or failed.
The Demo may separately confirm an overlapping Auto Highlight.
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
   confirmation showing the exact original and derivatives. The Clip record
   is marked deleted and each unshared file is removed only after successful
   file deletion. Preserve only the minimal provenance tombstone. If a
   file is referenced by another Clip, show that dependency and require a
   separate detach confirmation; never delete the shared artifact implicitly.
   The original is not recoverable in v1 after confirmed deletion.
5. **Export.** Export a selected Clip to a user-chosen ordinary file. The
   Discord option creates a local MP4 derivative using H.264 `yuv420p` video
   and AAC audio. It preserves source aspect ratio, caps output at 1920x1080
   and 60 fps, and targets no more than 10 MiB by default, matching [Discord's
   documented default upload limit](https://docs.discord.com/developers/reference#uploading-files).
   The target is user-configurable. The encoder calculates a total bitrate
   from the target, duration, and a 128 kbps AAC track, then verifies the
   resulting file size. The lowest allowed profile is 640x360 at 30 fps with
   400 kbps H.264 video and 64 kbps AAC audio. If that profile still exceeds
   the chosen ceiling, or the verified output exceeds it, export fails
   visibly and preserves the source. It never silently overshoots, uploads, or
   contacts Discord.

## Merged triggers

The Highlight rules define a 60-second Replay Buffer. An Auto Highlight signal
arrives at `round_end + 10s` and has a desired range from
`candidate_start - 15s` through `round_end + 10s`. A Manual Flag has the range
`flag_time - 15s` through `flag_time`, with no future post-roll.

Candidate merge is limited to triggers in the same round whose windows overlap
or whose starts are at most 10 seconds apart. It preserves every trigger
receipt and label and does not imply one raw file or one canonical Highlight.
After Demo reconciliation, the presentation may show multiple final labels
from contributing triggers, such as `multikill` and `clutch`, without forcing
one canonical label.
Raw Manual and later Auto saves remain independent; post-hoc consolidation is
allowed only when both files cover the desired union. Other triggers remain
separate Clips.

## V1 boundary

V1 includes local queue review, playback, one trim range, rename, favorite,
delete confirmation, deterministic candidate-merge presentation, and explicit
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

Each Clip has a review disposition of `saved`, `in_review`, `kept`, or
`deleted`, plus independent favorite and derivative records. Failed encoding,
missing source, or failed deletion remains visible with an actionable error and
does not silently change state. A Clip whose Capture Session ended before a
valid replay save is marked incomplete and cannot be presented as confirmed.

The dashboard distinguishes `Provisional` from `Demo confirmed` for Auto
Highlight evidence. Manual Flags show media-save status and any separate
overlapping Auto confirmation. If the Demo is unavailable, an Auto Highlight
remains playable and provisional. If parsing later fails, preserve it and its
failure Receipt rather than deleting or inventing confirmation.

## Interface requirements

- Issue 13 must provide the Capture Session identifier and lifecycle interface.
- Issue 16 must provide stable Demo Match and round identifiers for linking.
- gpu-screen-recorder output and host codec availability determine whether the
  Discord derivative can meet its size ceiling. This is a runtime capability
  and failure path, not a remaining product decision.
