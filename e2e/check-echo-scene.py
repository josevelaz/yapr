#!/usr/bin/env python3
"""Check a saved near-end scene before spending gateway credits.

The no-music scene must have a quiet system-audio reference before speech.
Does not capture, play, or transcribe audio; accepts float or PCM16 WAVs.
"""

import array
import json
import math
from pathlib import Path
import struct
import sys


def read_wav(path):
    data = path.read_bytes()
    offset = 12
    fmt = None
    while offset < len(data):
        kind = data[offset:offset + 4]
        size = struct.unpack_from("<I", data, offset + 4)[0]
        content = data[offset + 8:offset + 8 + size]
        if kind == b"fmt ":
            fmt, channels, rate = struct.unpack_from("<HHI", content)
            if fmt == 0xFFFE:
                fmt = struct.unpack_from("<H", content, 24)[0]
        if kind == b"data":
            values = array.array("f" if fmt == 3 else "h")
            values.frombytes(content)
            if fmt != 3:
                values = array.array("f", (value / 32768 for value in values))
            assert channels == 1 and rate == 16000
            return values
        offset += 8 + size + size % 2
    raise ValueError("missing WAV data")


scene = Path(sys.argv[1] if len(sys.argv) > 1 else "e2e/echo-results/no-music")
report = json.loads((scene / "measurement.json").read_text())
reference = read_wav(scene / "reference.wav")
mic = read_wav(scene / "off.wav")
offset = report["speech_offset_samples"]
region = reference[48000:offset - 3200]
power = math.sqrt(sum(value * value for value in region) / max(1, len(region)))
print(f"pre-speech reference RMS={power:.6f} ({20 * math.log10(max(power, 1e-12)):.2f} dBFS)")
print(f"delay={report['delay_ms']}ms; estimated correlation={report['delay_correlation']:.4f}")
print(f"dropped mic/reference={report['mic_dropped']}/{report['reference_dropped']}")
print(f"missing mic/reference samples={report['mic_missing']}/{report['reference_missing']}")
if report["music"] == "none":
    assert power < 0.0001, "no-music scene is contaminated by other system audio; repeat with other players quiet"
print("scene preflight passed")
