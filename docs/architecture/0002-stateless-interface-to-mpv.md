# 2. A stateless interface to mpv

Date: 2026-08-19

## Status

Accepted

## Context

mpvcast is an HTTP interface to a running mpv process. The queue, the
playing item, the position and the volume are all things mpv already tracks,
and the changes a client asks for are all things mpv already performs.

mpv's state changes without mpvcast asking. A file ends and the playlist
advances; yt-dlp resolves a title some seconds after a URL is loaded; a
keypress at the machine pauses playback. A server holding its own model of
any of this would be wrong after each of those events until it reconciled,
and a client would see the disagreement in the meantime.

Answering from mpv instead limits the API to what mpv models, and to how
promptly mpv reports.

## Decision

mpvcast will be a stateless interface to mpv. mpv holds the state. A request
to change something is translated into an mpv command, and a request to read
something is answered from what mpv has reported.

For the queue, this means mpv's playlist is the queue. The queue endpoints
translate to playlist commands, and the queue a client reads is a projection
of the `playlist` property.

Where mpv decides how playback behaves, we pass the request through rather
than pre-empting it. A seek past the end of a file is mpv's to interpret,
and whatever it makes of it becomes the state we report.

## Consequences

State cannot drift, because there is only one copy of it. A track ending, a
keypress at the machine and an HTTP request all reach a client the same way,
and there is no reconciliation logic anywhere in the server.

A client identifies the playing item by its index, because duplicate URLs in
a playlist are legal and position is the only thing that separates them.
Titles arrive late, since they come from mpv resolving a URL, so the API
reports them once they appear rather than promising them when an item is
queued.

Anything mpvcast displays that a client did not queue — an idle screen, a
notice, a logo — has to reach the window without becoming a playlist entry,
or it will appear in the queue and shift the index of everything after it.

When mpv stops answering, mpvcast has nothing of its own to serve. A failed
command reports a bad gateway rather than an internal error, because the
failure is in mpv rather than in the request or the server.
