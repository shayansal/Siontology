#!/usr/bin/env python3
"""A small Model Context Protocol server: notes kept as files in its
working directory. It speaks MCP (JSON-RPC 2.0, one message per line) over
standard input and output, with nothing but Python's standard library,
in both of its eras: requests that carry their protocol version (2026-07-28)
and the earlier handshake (2025-11-25 and before).

    sion agent mcp add notes --dir ./notes -- python3 examples/mcp-notes/server.py

Its tools say what they do in their annotations: listing and reading only
read local files, adding writes one, and deleting removes one.
"""

import json
import os
import re
import sys

MODERN = "2026-07-28"
LEGACY = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"]
NAME = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")

TOOLS = [
    {
        "name": "list_notes",
        "description": "List the notes kept, by name.",
        "inputSchema": {"type": "object", "properties": {}},
        "annotations": {"readOnlyHint": True, "openWorldHint": False},
    },
    {
        "name": "read_note",
        "description": "Read a note by its name.",
        "inputSchema": {
            "type": "object",
            "properties": {"name": {"type": "string"}},
            "required": ["name"],
        },
        "annotations": {"readOnlyHint": True, "openWorldHint": False},
    },
    {
        "name": "add_note",
        "description": "Keep a note under a name (lowercase letters, digits and hyphens), replacing one of that name.",
        "inputSchema": {
            "type": "object",
            "properties": {"name": {"type": "string"}, "text": {"type": "string"}},
            "required": ["name", "text"],
        },
        "annotations": {"openWorldHint": False, "idempotentHint": True},
    },
    {
        "name": "delete_note",
        "description": "Delete a note by its name.",
        "inputSchema": {
            "type": "object",
            "properties": {"name": {"type": "string"}},
            "required": ["name"],
        },
        "annotations": {"openWorldHint": False, "destructiveHint": True},
    },
]


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def result(id, value, modern):
    if modern:
        value = dict(value, resultType="complete")
    send({"jsonrpc": "2.0", "id": id, "result": value})


def error(id, code, message):
    send({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})


def text(value, is_error=False):
    return {"content": [{"type": "text", "text": value}], "isError": is_error}


def path(name):
    if not isinstance(name, str) or not NAME.match(name):
        return None
    return os.path.join(os.getcwd(), name + ".txt")


def call(name, args):
    if name == "list_notes":
        names = sorted(f[:-4] for f in os.listdir(".") if f.endswith(".txt"))
        return text("\n".join(names) if names else "(no notes)")
    if name in ("read_note", "add_note", "delete_note"):
        file = path(args.get("name"))
        if file is None:
            return text("a note's name is lowercase letters, digits and hyphens", True)
        if name == "add_note":
            with open(file + ".partial", "w", encoding="utf-8") as out:
                out.write(str(args.get("text", "")))
            os.replace(file + ".partial", file)
            return text("kept note " + args["name"])
        if not os.path.exists(file):
            return text("there is no note " + args["name"], True)
        if name == "read_note":
            with open(file, encoding="utf-8") as f:
                return text(f.read())
        os.remove(file)
        return text("deleted note " + args["name"])
    return None


for line in sys.stdin:
    if not line.strip():
        continue
    try:
        message = json.loads(line)
    except ValueError:
        continue
    method, id = message.get("method"), message.get("id")
    params = message.get("params") or {}
    version = (params.get("_meta") or {}).get("io.modelcontextprotocol/protocolVersion")
    modern = version is not None
    if modern and version != MODERN:
        error(id, -32022, "Unsupported protocol version")
    elif method == "server/discover":
        result(id, {
            "supportedVersions": [MODERN],
            "capabilities": {"tools": {}},
            "_meta": {"io.modelcontextprotocol/serverInfo": {"name": "notes", "version": "1.0"}},
        }, True)
    elif method == "initialize":
        asked = params.get("protocolVersion")
        result(id, {
            "protocolVersion": asked if asked in LEGACY else LEGACY[0],
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "notes", "version": "1.0"},
        }, False)
    elif method == "tools/list":
        result(id, {"tools": TOOLS}, modern)
    elif method == "tools/call":
        answer = call(params.get("name"), params.get("arguments") or {})
        if answer is None:
            error(id, -32602, "unknown tool " + str(params.get("name")))
        else:
            result(id, answer, modern)
    elif method == "ping":
        result(id, {}, modern)
    elif id is not None:
        error(id, -32601, "method not found: " + str(method))
