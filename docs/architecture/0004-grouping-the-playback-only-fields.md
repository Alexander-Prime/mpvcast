# 4. Grouping the playback-only fields

Date: 2026-08-19

## Status

Accepted

## Context

Four of the fields in `GET /status` exist only while an item is loaded: the
item itself, its index in the queue, the position and the duration. Volume,
mute and the error list apply whether or not anything is playing.

A client reading them as ordinary top-level fields has to check all four to
learn one thing, and nothing stops the document from reporting a position
with no current item.

mpv offers no single property that settles whether something is loaded.
While idle it reports a playlist position of -1, and its pause flag reads
false, which is the same value it carries during playback. The answer has to
be derived and then applied the same way to every field that depends on it.

## Decision

The four playback-only fields are grouped into a `playback` object, which is
null when mpv is idle. Whether anything is loaded is decided once, where
that object is built, and every field inside it follows from that.

```ts
{
  "playback": { /* the fields that need a loaded item */ } | null
  "volume": number
  "muted": boolean
  "errors": Error[]
}
```

This is the practice Yaron Minsky named [making illegal states
unrepresentable][minsky]: a position with no current item is not a
combination the document can express, so nothing has to check for it.

## Consequences

A client makes one check rather than four, and a position belonging to a
finished track has nowhere to be reported from.

The JSON mirrors the Rust type, so serialisation is derived. Keeping the
fields flat in JSON while grouping them in the type would need a serialiser
written by hand, because `serde`'s `flatten` omits the keys of an absent
object rather than writing them as null.

A field added later belongs inside the object if it means nothing while
idle, and beside it if it applies either way.

[minsky]: https://blog.janestreet.com/effective-ml/
