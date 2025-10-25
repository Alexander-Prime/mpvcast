# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

mpvcast is a lightweight REST API server for remote control of mpv media player, designed for "share URL from phone → play on TV" workflows. Target platform is NixOS with support for YouTube, Twitch, Nebula, and 1000+ sites via yt-dlp.

## Build and Run Commands

```bash
# Build the project
cargo build

# Run in development
cargo run

# Run tests
cargo test

# Build release binary
cargo build --release

# Check without building
cargo check
```

## Core Architecture

### mpv Integration Pattern
- Use mpv's JSON IPC interface via `--input-ipc-server` socket for control
- Launch mpv in fullscreen with hardware decoding flags for TV playback
- Handle mpv process lifecycle (spawn, monitor, graceful shutdown)
- Communicate via Unix socket (typically `/tmp/mpv-socket`)

### Server Design
- HTTP REST API server listening on local network
- Must handle multiple concurrent requests safely with proper state management
- Manages playback state and optional queue
- Spawns and controls mpv process instances

### Security Model
- Listen only on local network (NOT internet-exposed)
- Input validation required for all URLs and file paths
- Optional simple token-based authentication

## Key Requirements

**External Dependencies:**
- mpv (media player binary)
- yt-dlp (streaming support)

**Core Endpoints to Implement:**
- POST /play - Accept URL or file path
- POST /pause, /stop
- POST /seek (forward/backward)
- POST /volume
- GET /status - Current playback state
- Optional: Queue management (/queue/add, /queue/list, /queue/clear, /queue/next)

## NixOS Packaging Considerations

When ready for deployment, package as NixOS service with:
- systemd service definition
- Automatic startup capability
- Configuration via NixOS module options (listen address/port, mpv path, auth token, etc.)

## Development Philosophy

From MPV_REMOTE_README.md:
- Keep dependencies minimal
- Prioritize reliability over features
- Handle mpv crashes gracefully
- Provide clear error messages for unsupported URLs
