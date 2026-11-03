# mcp-notes

A small [Model Context Protocol](https://modelcontextprotocol.io) server that
keeps notes as files, written in Python with the standard library only. It is
the example server of the agent runtime's tool pages.

Part of [Siontology](../../README.md).

## What the server does

`server.py` speaks MCP (JSON-RPC 2.0, one message per line) over standard
input and output. It answers in both eras of the protocol: requests that carry
their protocol version (2026-07-28) and the earlier handshake (2025-11-25 and
before). It keeps each note as `<name>.txt` in its working directory, and a
note's name is lowercase letters, digits and hyphens. It offers four tools,
and each says what it does in its annotations:

| Tool | What it does | Annotations |
|---|---|---|
| `list_notes` | Lists the notes kept, by name | read-only, closed world |
| `read_note` | Reads a note by its name | read-only, closed world |
| `add_note` | Keeps a note under a name, replacing one of that name (written to a `.partial` file, then renamed) | idempotent, closed world |
| `delete_note` | Deletes a note by its name | destructive, closed world |

## How to run it

The runtime starts the server itself. From the repository root, with notes
kept in a directory called `notes`:

```text
sion agent mcp add notes --dir ./notes -- python3 examples/mcp-notes/server.py
```

That is the command in the server's own docstring. The book shows it, with
the output, in
[Tools from MCP servers](../../docs/book/src/agent/tools.md): `sion agent mcp
add` starts the server, lists its tools and shows, for each, what its calls
are taken to do. A session then gets the tools with `--mcp notes`, each named
`<server>_<tool>` (`notes_add_note`, `notes_list_notes`, and so on). The
server runs as you, outside any sandbox, unless you add `--sandbox bwrap` (or a
Docker image).

## What it shows

- **Pinning.** The tools it offers are pinned: the runtime keeps a digest of
  each tool's name, description, input schema and annotations as they were at
  the review.
- **Effects from annotations.** A tool that only reads local data is a read;
  a tool that does not say it is closed-world would be external, and what it
  returns would taint the session. Here, `list_notes` and `read_note` are
  reads, and `add_note` and `delete_note` are writes. `--effect TOOL=spend`
  (or `read`, `write`, `external`) overrides what a tool says of itself.
- **Journaling.** Calls that are neither read-only nor idempotent are
  journaled, so an identical call is not made twice: `delete_note` is, and
  `add_note` is marked idempotent and repeats run again.
- **Uncertain calls.** A call that got no answer may have taken effect, so it
  is not repeated.

## Read more

- [Tools for agents](../../docs/book/src/agent/tools.md)
- [MCP reference](../../docs/book/src/reference/mcp.md)
- [Architecture](../../ARCHITECTURE.md)
