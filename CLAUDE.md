# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

mpvcast is a lightweight REST API server for remote control of mpv media player, designed for "share URL from phone → play on TV" workflows. Target platform is NixOS with support for YouTube, Twitch, Nebula, and 1000+ sites via yt-dlp.

## Build and Run Commands

```bash
cargo build
cargo run
cargo test
cargo build --release
cargo check
```

## Core Architecture

The reasoning behind these choices, and the alternatives that were
rejected, is recorded in `docs/architecture/`.

### mpv Integration

mpvcast spawns the system `mpv` binary as a long-lived child process and
controls it over mpv's JSON IPC socket. The mpv binary has `ytdl_hook.lua`
wired in, which is what gives YouTube / Twitch / Nebula support via yt-dlp.

The mpv child is spawned at server startup with:

```
mpv --idle=yes --force-window=immediate
    --input-ipc-server=<socket>
    --fullscreen --hwdec=auto-safe --ytdl=yes
```

This produces a window that's up immediately on the TV, hardware-accelerated
video, an idle mpv that survives between playback sessions, and yt-dlp URL
resolution handled inside mpv.

### IPC Client

A hand-rolled tokio client (`src/mpv/`) talks to mpv over the Unix socket:

- Newline-delimited JSON in both directions.
- Outgoing commands carry a `request_id`; replies are correlated back via a
  pending-request map and `tokio::sync::oneshot` channels.
- Asynchronous mpv events (`end-file`, property changes, etc.) are routed to
  a `tokio::sync::broadcast` channel so multiple subscribers can observe
  them.
- The client exposes typed helpers for the commands mpvcast uses
  (`get_property`, `set_property`, `observe_property`, `loadfile`,
  playlist manipulation, `seek`, etc.) on top of a generic `send`.

### Status Snapshot

`MpvClient` observes the properties that back the API surface (`pause`,
`time-pos`, `duration`, `volume`, `mute`, `playlist`, `playlist-pos`,
`media-title`) and maintains a cached snapshot updated by mpv
`property-change` events. `GET /status`, `GET /queue`, and the SSE `/events`
stream all read from that snapshot. `AppState` is `Arc<MpvClient>`.

### Server Design

Axum HTTP server. Handlers are thin shims over `MpvClient` calls. Binds to
`0.0.0.0` for LAN reachability from phones on the local network.

The port defaults to 4590 and is overridden with `--port <number>`. The
address it settled on is logged at startup. Any other argument is an
error, so a mistyped flag can't leave the server listening somewhere
unexpected.

## HTTP API

State surface:

```
GET    /status      current playback state (schema below)
PATCH  /status      update either or both of `volume` (0-100) and `muted` (bool)
```

`GET /status` response:

```json
{
  "playback": {
    "paused": boolean,
    "current": { "url": string, "title": string | null },
    "current_index": integer,
    "position": float | null,
    "duration": float | null
  } | null,
  "volume": integer,
  "muted": boolean,
  "errors": [{ "message": string }]
}
```

- `playback`: everything that only means something while an item is
  loaded, grouped so it is absent as a unit. `null` when mpv is idle, so
  a client has no stale position to read rather than a set of nulls to
  remember to check.
- `paused`: whether the loaded item is stopped.
- `current`: the loaded item. `title` is sourced from media metadata and
  is `null` until it resolves.
- `current_index`: position of the current item in the queue.
- `position`, `duration`: seconds. Each is `null` until mpv resolves it,
  and `duration` stays `null` for streams with unknown length.
- `volume`: integer 0–100, clamped at the API boundary (mpv's internal
  range can exceed 100).
- `muted`: whether audio output is silenced. Independent of `volume` —
  unmuting restores audio at the existing volume setting.
- `errors`: playback errors reported by mpv (unresolvable URL, decoder
  failure, etc.). Cleared when a new item begins playing.

`PATCH /status` carries the two settings that mean something whether or
not an item is loaded. Everything nested under `playback` is changed
through the transport routes below instead.

Queue resource (mpv playlist):

```
GET    /queue       list items
PUT    /queue       replace: stop + playlist-clear + loadfile each
POST   /queue       append: loadfile
DELETE /queue       clear: stop + playlist-clear
```

`GET /queue` returns an array:

```json
[
  { "url": string, "title": string | null },
  ...
]
```

`title` is server-derived from media metadata; it's `null` until the
entry has been loaded by mpv.

`PUT /queue` accepts an array of `{ "url": string }`. `POST /queue`
accepts a single `{ "url": string }`. The client never supplies `title`.

`loadfile` carries mpv's `append-play` flag, so adding a URL starts an
idle player and leaves an item that's already playing alone. Sharing a
link from a phone is therefore one request rather than two.

Transport actions:

```
POST   /playback/play       set pause=false; if idle and queue non-empty, start at index 0
POST   /playback/pause      set pause=true
POST   /playback/next       playlist-next
POST   /playback/previous   playlist-prev
```

Playback position:

```
PUT    /playback/position   seek to an absolute position (body: {"seconds": float})
PATCH  /playback/position   seek by a delta (body: {"delta": float}); positive = forward, negative = back
```

The delta form maps onto mpv's relative seek, so a skip button never has
to compute a target from a position that moves underneath it. The
absolute form is what a scrubber drag lands on.

Both respond `409` when `playback` is `null`, since there is nothing to
seek within.

Endpoints that only act return `204 No Content`. When mpv refuses a
command or stops answering, the response is `502 Bad Gateway`.

Real-time updates:

```
GET    /events      Server-Sent Events stream; emits the current status
                    snapshot (same shape as `GET /status`) whenever it
                    changes
```

## External Dependencies

- `mpv` (media player binary) on PATH at runtime.
- `yt-dlp` — pulled in transitively by NixOS's `mpv` package; mpv invokes it.

## Development Philosophy

- Keep dependencies minimal.
- Prioritize reliability over features.
