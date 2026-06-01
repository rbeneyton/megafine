"""megafine region-timing instrumentation (Python).

Bracket the region you want timed with ``megafine_start()`` / ``megafine_stop()``.
The calls are no-ops unless the program is run under ``megafine --region``
(which sets ``MEGAFINE_FD`` in the environment), so the instrumented script
still runs normally on its own.

    from megafine import megafine_start, megafine_stop
    ...
    megafine_start()
    # ... code to measure ...
    megafine_stop()
"""

import os
import struct
import time

# Resolved once: the inherited write fd, or None for a standalone run.
_FD = next(iter([int(s) for s in [os.environ.get("MEGAFINE_FD")] if s]), None)

# 9-byte event: [tag:u8][ns:u64 native-endian]. 9 < PIPE_BUF, so the write is
# atomic and safe from multiple threads. native byte order, no padding.
_EVENT = struct.Struct("=BQ")


def _emit(tag):
    if _FD is None:
        return  # standalone run: no-op, no I/O
    # Non-owning write; never closes the inherited fd.
    os.write(_FD, _EVENT.pack(tag, time.monotonic_ns()))


def megafine_start():
    _emit(0)


def megafine_stop():
    _emit(1)
