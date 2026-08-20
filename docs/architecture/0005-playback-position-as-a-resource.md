# 5. Playback position as a resource

Date: 2026-08-20

## Status

Accepted

## Context

There are two ways to move the playhead. A skip button moves by a fixed
amount from wherever playback has reached; dragging the scrubber moves to a
particular point in the file.

One endpoint could take both forms, with a body carrying either a delta or
an absolute point. It would then have to reject the requests that set
neither or both, and the documentation would have to explain the rule.

## Decision

The playback position is a resource at `/playback/position`. `PUT` sets it,
taking `{"seconds": float}`. `PATCH` adjusts it, taking `{"delta": float}`,
and maps onto mpv's relative seek.

Both respond `409` when nothing is loaded, since there is no position
without a loaded item (ADR 4).

Values reach mpv unchanged, even out of bounds. mpvcast does not clamp or
otherwise interpret them (ADR 2).

## Consequences

Each request body has one required field, so a missing or unrecognised field
is rejected while deserialising rather than by code written to check for it.

The method distinguishes setting from adjusting, so neither body needs a
field saying which was meant.

These are the only endpoints that depend on something being loaded. Volume
and mute apply to an idle player, and the transport actions either apply or
are refused by mpv.
