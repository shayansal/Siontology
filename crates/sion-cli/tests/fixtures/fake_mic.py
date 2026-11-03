#!/usr/bin/env python3
"""A stand-in for a microphone, for the live voice tests: raw 16-bit mono
samples at 16 kHz on standard output, written as fast as they would be
spoken, following a plan given as arguments:

  silence:<seconds>      quiet (a faint, steady hiss)
  tone:<seconds>         something said: a loud 300 Hz tone
  wait:<file>[=<text>]   quiet until the file exists (and holds the text),
                         for 30 seconds at most

It ends when the plan does.
"""

import math
import os
import random
import struct
import sys
import time

RATE = 16000
CHUNK = 320  # 20 ms

out = sys.stdout.buffer
noise = random.Random(7)
start = time.monotonic()
written = 0
phase = 0


def emit(loud):
    global written, phase
    if loud:
        samples = [
            int(8000 * math.sin(2 * math.pi * 300 * (phase + i) / RATE)) for i in range(CHUNK)
        ]
        phase += CHUNK
    else:
        samples = [noise.randint(-60, 60) for _ in range(CHUNK)]
    out.write(struct.pack("<%dh" % CHUNK, *samples))
    out.flush()
    written += CHUNK
    ahead = written / RATE - (time.monotonic() - start)
    if ahead > 0:
        time.sleep(ahead)


def holds(path, text):
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            return text in f.read()
    except OSError:
        return False


try:
    for step in sys.argv[1:]:
        kind, _, rest = step.partition(":")
        if kind in ("silence", "tone"):
            for _ in range(round(float(rest) * RATE / CHUNK)):
                emit(kind == "tone")
        elif kind == "wait":
            path, _, text = rest.partition("=")
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline and not holds(path, text):
                emit(False)
        else:
            sys.exit("unknown step: " + step)
except BrokenPipeError:
    pass
