# 1. Record architecture decisions

Date: 2026-08-18

## Status

Accepted

## Context

Decisions about how this project is built are argued through once and then
disappear. What survives is the result: the code, and documentation
describing how the system currently behaves. Within a few months a choice
that was weighed carefully against alternatives is indistinguishable from an
accident, and revisiting it means reconstructing the forces from scratch.

Documentation addressed to a particular tool cannot fill the gap. It
describes the present state for one consumer, and it changes whenever that
tool's conventions do.

## Decision

We will keep a log of architecture decisions here, one file per decision,
numbered sequentially from 0001 and never reusing a number. Each follows
[Michael Nygard's format][nygard]: a short noun phrase for a title, a
status, the context that created the pressure, the decision stated in the
first person plural, and the consequences that follow. They are written in
full sentences for whoever reads the project next, and their length follows
the complexity of the decision.

An accepted record is immutable. A reversed decision keeps its record and is
marked superseded by the number that replaces it.

## Consequences

Reasoning outlives the conversation that produced it, so a later reader can
tell a considered trade-off from an accident and can disagree with a
decision knowing what it was weighed against.

The account of the architecture stays put when tooling changes.

Deciding which choices deserve a record is a judgement that will sometimes
be made wrongly. Writing one is cheap enough that erring toward writing it
is the safer mistake.

[nygard]: https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions.html
