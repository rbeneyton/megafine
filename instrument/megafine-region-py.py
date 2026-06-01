#!/usr/bin/env python3
"""Ancillary script to exercise `megafine --region`.

Takes three sleep durations (seconds) and brackets the middle one with the
region markers, so `megafine --region` should report ≈ the 2nd value:

    sleep(before); megafine_start(); sleep(region); megafine_stop(); sleep(after)

Usage: megafine-region-py.py <before> <region> <after>
"""

import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from megafine import megafine_start, megafine_stop

if len(sys.argv) != 4:
    sys.exit("usage: megafine-region-py.py <before> <region> <after>  (seconds)")
before, region, after = (float(x) for x in sys.argv[1:4])

time.sleep(before)
megafine_start()
time.sleep(region)
megafine_stop()
time.sleep(after)