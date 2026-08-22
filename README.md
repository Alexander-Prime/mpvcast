# mpvcast

A small HTTP server for driving [mpv](https://mpv.io) on another machine.
Share a link from your phone and it plays on the TV. Anything yt-dlp can
resolve will play, which covers YouTube, Twitch, Nebula and around a
thousand other sites.

mpvcast starts mpv as a child process and controls it over mpv's JSON IPC
socket, so the player is a real mpv running your own configuration.

## Requirements

- `mpv` on `PATH`. mpvcast runs the system binary rather than linking a
  library, and will not start without it.
- `yt-dlp`, which mpv invokes to resolve stream URLs. On NixOS the `mpv`
  package brings this along.
- A graphical session for mpv to open a window in. mpvcast asks for
  fullscreen on the display mpv finds.

## Running it

```bash
cargo run
```

The server listens on port 4590 across all interfaces, so a phone on the
same network can reach it. Pass `--port` to change that:

```bash
cargo run -- --port 8080
```

The address it settled on is logged at startup.

## Architecture

Decisions about how mpvcast is built, and the alternatives that were
rejected, are recorded in [`docs/architecture/`](docs/architecture/).
