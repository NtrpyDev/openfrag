# Issue tracker conventions

This repo's tracker is GitHub Issues on `NtrpyDev/openfrag`. Use the `gh` CLI.

## Wayfinding operations

The wayfinder map and its tickets live here as ordinary GitHub issues.

- **The map** is the single issue labeled `wayfinder:map` (issue #1). Load it first each session.
- **Tickets** are child issues of the map: any open issue carrying a `wayfinder:<type>` label (`research`, `prototype`, `grilling`, `task`). No other parent link is used; the label is membership.
- **Claiming**: add the `wayfinder:claimed` label before doing any work on a ticket, so concurrent sessions skip it.
- **Blocking**: a ticket that depends on others starts its body with a line `Blocked-by: #N #M`. A ticket is unblocked when every listed issue is closed. Tickets without that line are unblocked.
- **The frontier** (what to work next): open issues with a `wayfinder:<type>` label, without `wayfinder:claimed`, whose Blocked-by issues (if any) are all closed.

  ```sh
  gh issue list --label wayfinder:research,wayfinder:prototype,wayfinder:grilling,wayfinder:task \
    --state open --json number,title,labels,body
  # then filter out claimed ones and any with an open Blocked-by reference
  ```

- **Resolving a ticket**: post the answer as a comment, close the issue, then append one line to the map's "Decisions so far" section: `- [<ticket title>](<url>) - <one-line gist>`. Graduate any fog the answer unlocks into new tickets and remove those patches from the Fog section.

## Repo conventions

- Every file and folder must show a unique, human, one-sentence description in GitHub's file list. Commit each new file separately with that description as the commit message, and check for duplicate descriptions before every push.
- No em dashes anywhere: code comments, docs, issues, UI copy.
