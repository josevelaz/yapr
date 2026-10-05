#!/usr/bin/env python3
"""Generate deterministic, loud music-like interference without vocals or clipping.

Run from any directory. Does not play audio or change system volume.
"""

import math
from pathlib import Path
import random
import struct
import wave

RATE = 16000
SECONDS = 30
rng = random.Random(42)
signal = []
for i in range(RATE * SECONDS):
    t = i / RATE
    root = [130.81, 174.61, 146.83, 196.0][int(t / 2) % 4]
    chord = sum(
        math.sin(math.tau * root * ratio * h * t) / h
        for ratio in (1.0, 1.259921, 1.498307)
        for h in range(1, 7)
    ) * 0.1
    noise = rng.uniform(-1, 1)
    beat = (t * 2) % 1
    kick = math.sin(math.tau * (55 * t - 0.05 * math.exp(-beat * 25))) * math.exp(-beat * 16) * 0.3
    snare = noise * math.exp(-((t * 2 + 0.5) % 1) * 24) * 0.4
    hat = noise * math.exp(-((t * 8) % 1) * 35) * 0.1
    signal.append(chord + kick + snare + hat)
peak = max(map(abs, signal))
signal = [sample * 0.97 / peak for sample in signal]
path = Path(__file__).resolve().parent / "fixtures/echo-music.wav"
with wave.open(str(path), "wb") as wav:
    wav.setnchannels(1)
    wav.setsampwidth(2)
    wav.setframerate(RATE)
    wav.writeframes(b"".join(struct.pack("<h", round(sample * 32767)) for sample in signal))
rms = math.sqrt(sum(sample * sample for sample in signal) / len(signal))
print(f"{path.name}: {SECONDS}s, 16 kHz PCM16, peak=0.97, RMS={rms:.4f} ({20 * math.log10(rms):.2f} dBFS)")
