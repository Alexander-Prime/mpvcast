# 3. mpv as a child process driven over JSON IPC

Date: 2026-08-19

## Status

Accepted

## Context

Controlling mpv from a server can be done two ways. The server can link
libmpv and hold a player inside its own process, or it can run the mpv
binary as a child and talk to it over the JSON IPC socket mpv exposes.

mpvcast was first built the first way. It linked `libmpv2` and configured a
headless player with video disabled, and around that sat modules the server
had to own: a queue of its own, and a resolver that turned a URL into a
stream to hand to the player.

libmpv is a C library, so linking it makes a C toolchain and libmpv headers
a prerequisite for building the server at all. The mpv binary needs neither;
it is found on `PATH` when the server starts.

mpvcast exists to put video on a television. The mpv binary already handles
display setup, fullscreen, hardware decoding and yt-dlp integration through
command-line flags, where reaching the same behaviour through libmpv means
setting it property by property from inside the server.

A player linked into the server runs in its address space, so a decoder or
driver fault ends the process serving HTTP.

## Decision

We will spawn the system mpv binary at server startup and control it over
mpv's JSON IPC socket, with newline-delimited JSON in both directions.

The flags passed at spawn configure the player: the window, fullscreen,
hardware decoding, and yt-dlp resolution through mpv's `ytdl_hook`. The
socket carries commands and the events mpv emits in response.

## Consequences

Building mpvcast needs only a Rust toolchain. mpv becomes a runtime
dependency discovered on `PATH` rather than something linked at compile
time, and the NixOS package supplies yt-dlp along with it.

Resolving a URL is mpv's work. The resolver module the first design needed
was deleted along with the queue, because both were reimplementations of
things the mpv binary already does.

A fault inside mpv takes down the player rather than the server. mpvcast
keeps serving and reports that its commands are failing.

Commands and their replies travel as separate messages over one socket, so
mpvcast tags each command with an identifier, matches the reply carrying
that identifier back to whoever is waiting, and separates mpv's unsolicited
events from both. libmpv would have provided this as function calls that
return their results.

Everything mpvcast can do is something expressible as an mpv command or
property. The IPC surface is the whole of what the server can reach.
