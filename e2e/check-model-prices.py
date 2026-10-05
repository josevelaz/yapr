#!/usr/bin/env python3

import json
from pathlib import Path
import re
import subprocess
import sys

APP = Path.home() / "Applications/Yapr.app/Contents/MacOS/Yapr"
run = subprocess.run([str(APP), "--list-models"], capture_output=True, text=True, timeout=60, check=True)
models = [json.loads(line) for line in run.stdout.splitlines()]
failures = []


def close(text, expected):
    return abs(float(text.replace("$", "")) - expected) <= abs(expected) * 0.03 + 0.0001


def number(value):
    try:
        return float(value) if value not in (None, "") else None
    except ValueError:
        return None


def verify(model):
    p = model["pricing"]
    text = model["priceText"]
    dollars = re.findall(r"\$[\d.]+", text)
    per_second = number(p.get("transcription_duration_cost_per_second"))
    if model["transcription"] and per_second is not None:
        if per_second == 0:
            return text == "free"
        return text.endswith("/min") and len(dollars) == 1 and close(dollars[0], per_second * 60)
    audio_in = number(p.get("audio_input_token_cost"))
    input_ = (audio_in if audio_in is not None else number(p.get("input"))) if model["transcription"] else number(p.get("input"))
    output = number(p.get("output"))
    if input_ is None and output is None:
        return text == "price not listed"
    if not input_ and not output:
        return text == "free"
    expected = [v * 1e6 for v in (input_, output) if v is not None]
    return text.endswith("per 1M tokens") and len(dollars) == len(expected) and all(close(d, e) for d, e in zip(dollars, expected))


def verify_short(model):
    full, short = model["priceText"], model["priceShort"]
    if len(short) > 16 or short not in model["menuTitle"]:
        return False
    if full == "price not listed":
        return short == "no price"
    if full == "free":
        return short == "free"
    if full.endswith("/min"):
        return short == full
    dollars = re.findall(r"\$[\d.]+", full)
    return short == (" / ".join(dollars) if len(dollars) == 2 else full.replace(" per 1M tokens", ""))


for model in models:
    ok = verify(model) and verify_short(model)
    if not ok:
        failures.append(model["id"])
    if model["transcription"] or not ok:
        print(f"{'ok  ' if ok else 'FAIL'} {model['menuTitle']}")

ranks = {"all": 0, "some": 1}
transcription = [m for m in models if m["transcription"]]
order = [ranks.get(m["zdr"], 2) for m in transcription]
if order != sorted(order):
    failures.append("zdr order")
if not any(m["streaming"] for m in transcription):
    failures.append("no streaming model found")
print(f"\nChecked {len(models)} models ({len(transcription)} transcription), {len(failures)} failures: {failures}")
sys.exit(1 if failures else 0)
