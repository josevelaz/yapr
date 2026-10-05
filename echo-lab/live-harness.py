#!/usr/bin/env python3
"""Bounded live E2E run. No playback starts until both capture streams start."""
import pathlib
import select
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parent
seconds = float(sys.argv[1]) if len(sys.argv) > 1 else 22
command = ["timeout", f"{seconds + 40}s", "target/release/live", str(seconds), "--request-permission", *sys.argv[2:]]
players = []
with (ROOT / "out/live.log").open("w") as log:
    recorder = subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1)
    try:
        ready = False
        # Read one character stream through unbuffered fd to avoid select/text buffering races.
        deadline = time.monotonic() + 35
        buffer = b""
        import os
        while time.monotonic() < deadline:
            if select.select([recorder.stdout], [], [], 0.1)[0]:
                data = os.read(recorder.stdout.fileno(), 4096)
                if not data:
                    break
                buffer += data
                text = data.decode(errors="replace")
                print(text, end="", flush=True)
                log.write(text)
                log.flush()
                if b"READY\n" in buffer:
                    ready = True
                    break
        if not ready:
            if recorder.poll() is None:
                recorder.terminate()
            recorder.wait(timeout=5)
            sys.exit(recorder.returncode or 1)
        players.append(subprocess.Popen(["afplay", "-v", "1.0", "out/play-music.wav"], cwd=ROOT))
        time.sleep(6)
        if recorder.poll() is None:
            players.append(subprocess.Popen(["afplay", "-v", "1.0", "../e2e/fixtures/pauses.wav"], cwd=ROOT))
        output, _ = recorder.communicate(timeout=seconds + 10)
        print(output, end="")
        log.write(output)
        sys.exit(recorder.returncode)
    finally:
        for player in players:
            if player.poll() is None:
                player.terminate()
            player.wait(timeout=5)
        if recorder.poll() is None:
            recorder.kill()
            recorder.wait(timeout=5)
