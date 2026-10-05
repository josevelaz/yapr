#!/usr/bin/env python3

import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent
APP = Path.home() / "Applications/Yapr.app/Contents/MacOS/Yapr"
OUT = ROOT / "e2e/transcription-results"
OUT.mkdir(exist_ok=True)
FIXTURE = str(ROOT / "e2e/fixtures/pauses.wav")
MAI = "microsoft/mai-transcribe-2"
GEMINI = "google/gemini-3.5-flash-lite"
results = []


def dictate(name, *args, proxy=False):
    env = os.environ.copy()
    env["YAPR_SUPPORT_DIR"] = SUPPORT
    if proxy:
        env["HTTPS_PROXY"] = env["https_proxy"] = "http://127.0.0.1:9"
    run = subprocess.run([str(APP), "--dictate", *args], env=env, capture_output=True, text=True, timeout=240)
    assert run.returncode == 0, f"{name}: exit {run.returncode}: {run.stderr.strip()}"
    reply = json.loads(run.stdout.strip().splitlines()[-1])
    results.append({"name": name, "reply": reply})
    print(json.dumps(results[-1]), flush=True)
    return reply


with tempfile.TemporaryDirectory() as SUPPORT:
    try:
        reply = dictate("missing-key", FIXTURE, "--model", MAI, "--cleanup", "none", "--missing-key")
        assert "No AI Gateway API key" in reply["startError"]

        reply = dictate(
            "cleanup", FIXTURE, "--model", MAI, "--cleanup", GEMINI,
            "--instructions", "Write README in uppercase. Treat C:\\notes as a literal path.",
        )
        assert reply["raw"] and reply["cleaned"] and "README" in reply["text"]
        assert reply["streamed"] is False and "first_cleanup_text_ms" in reply["timings"]

        reply = dictate("rest-fallback", FIXTURE, "--model", MAI, "--cleanup", "none", "--streaming")
        assert reply["raw"] and reply["text"] == reply["raw"] and reply["streamed"] is False and reply["error"] is None

        reply = dictate("bad-cleanup-fallback", FIXTURE, "--model", MAI, "--cleanup", "invalid/not-a-model")
        assert reply["raw"] and reply["text"] == reply["raw"] and not reply["cleaned"] and "not found" in reply["error"]

        reply = dictate(
            "network-cleanup-fallback", FIXTURE, "--model", "spacexai/grok-stt", "--streaming",
            "--cleanup", GEMINI, proxy=True,
        )
        assert reply["raw"] and reply["text"] == reply["raw"] and reply["streamed"] and not reply["cleaned"]
        assert "Could not reach" in reply["error"]

        reply = dictate("microphone", "mic", "--model", MAI, "--cleanup", "none", "--seconds", "1.5")
        assert reply["error"] is None

        reply = dictate("echo-denied", "mic", "--model", MAI, "--cleanup", "none", "--seconds", "1", "--deny-system-audio")
        assert reply["echo"]["active"] is False
        assert "allow Screen & System Audio Recording" in reply["echo"]["reason"]
        print("dictation flow assertions passed", flush=True)
    finally:
        (OUT / "dictation-flow.json").write_text(json.dumps(results, indent=2) + "\n")
