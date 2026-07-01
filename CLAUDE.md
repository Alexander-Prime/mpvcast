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

### State Ownership

mpv's internal playlist is the source of truth for what's queued and what's
playing. `MpvClient` observes the properties that back the API surface
(`pause`, `time-pos`, `duration`, `volume`, `mute`, `playlist`,
`playlist-pos`, `media-title`) and maintains a cached snapshot updated by
mpv `property-change` events. `GET /status`, `GET /queue`, and the SSE
`/events` stream all read from that snapshot rather than issuing fresh
`get_property` calls per request. `AppState` is `Arc<MpvClient>`.

### Server Design

Axum HTTP server. Handlers are thin shims over `MpvClient` calls. Binds to
`0.0.0.0` for LAN reachability from phones on the local network.

## HTTP API

State surface:

```
GET    /status      current playback state (schema below)
PATCH  /status      update any subset of `position` (seconds, absolute seek), `volume` (0-100), and `muted` (bool)
```

`GET /status` response:

```json
{
  "state": "idle" | "paused" | "playing",
  "current": { "url": string, "title": string | null } | null,
  "current_index": integer | null,
  "position": float | null,
  "duration": float | null,
  "volume": integer,
  "muted": boolean,
  "errors": [{ "message": string }]
}
```

- `state`: `idle` when nothing is loaded, `paused` when a track is loaded
  but stopped, `playing` otherwise.
- `current`: the loaded item; `null` when idle. `title` inside it is
  sourced from media metadata and is `null` until it resolves.
- `current_index`: position of the current item in the queue; `null` when
  idle. Disambiguates which entry is playing when the queue contains
  duplicate URLs.
- `position`, `duration`: seconds. Both `null` when idle; `duration` is
  also `null` for streams with unknown length.
- `volume`: integer 0–100, clamped at the API boundary (mpv's internal
  range can exceed 100).
- `muted`: whether audio output is silenced. Independent of `volume` —
  unmuting restores audio at the existing volume setting.
- `errors`: playback errors reported by mpv (unresolvable URL, decoder
  failure, etc.). Cleared when a new item begins playing.

`PATCH /status` with `position` while `state` is `idle` is a no-op.

Queue resource (mpv playlist):

```
GET    /queue       list items
PUT    /queue       replace: stop + playlist-clear + loadfile each append
POST   /queue       append: loadfile each append
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

Transport actions:

```
POST   /playback/play       set pause=false; if idle and queue non-empty, start at index 0
POST   /playback/pause      set pause=true
POST   /playback/next       playlist-next
POST   /playback/previous   playlist-prev
POST   /playback/seek       seek by delta seconds (body: {"delta": float}); positive = forward, negative = back
```

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
