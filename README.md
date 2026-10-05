# Yapr

A macOS menu bar app for dictation. Press a shortcut and speak. A floating bar shows that it is
listening (and your words as you speak, with a streaming model). Press the shortcut again: the
recording is transcribed through Vercel AI Gateway, cleaned up by a language model of your choice,
and pasted into the app you were using. Music or other sound playing on your Mac is removed from
the recording first.

```text
shortcut (⌥Space) or menu ──► Yapr.app (Rust, AppKit)
                              ├─ microphone ─┐
                              ├─ system audio (ScreenCaptureKit) ─► WebRTC echo canceller ─► 16 kHz mono
                              ├─ AI Gateway transcription
                              │    streaming model:  WebSocket while you speak (live words on the bar)
                              │    other models:     whole recording sent when you stop
                              ├─ AI Gateway cleanup (/v1/chat/completions, streamed into the bar)
                              ├─ floating bar (never takes focus)
                              └─ clipboard + ⌘V into the front app
```

## Requirements

- macOS 14 or later on Apple Silicon
- A [Vercel AI Gateway](https://vercel.com/docs/ai-gateway) API key
- To build from source: Rust (`cargo`) and Homebrew `meson` and `ninja` (they build the WebRTC
  audio library): `brew install meson ninja`

## Download

With Homebrew:

```sh
brew tap josevelaz/yapr https://github.com/josevelaz/yapr
brew install --cask josevelaz/yapr/yapr
```

Or get `Yapr-X.Y.Z.zip` from [Releases](https://github.com/josevelaz/yapr/releases), unzip it, and
move `Yapr.app` to Applications. Either way, open it once. Releases are not notarized by Apple, so
macOS blocks the first launch: choose System Settings → Privacy & Security → Open Anyway, or run
`xattr -dr com.apple.quarantine /Applications/Yapr.app`. Then follow step 3 below. Maintainers:
see [docs/releases.md](docs/releases.md).

## Build from source

1. **Create a local signing certificate** (once). macOS ties microphone, system audio,
   Accessibility and Keychain access to the app's signature; a stable certificate keeps those
   grants across rebuilds. Without it the build signs ad-hoc and macOS asks again after every
   rebuild.

   ```sh
   scripts/make-signing-cert.sh
   ```

2. **Build and install** to `~/Applications/Yapr.app`, then open it:

   ```sh
   scripts/build-app.sh
   open ~/Applications/Yapr.app
   ```

3. **Finish setup from the menu bar icon.** The Setup section lists what is missing; click a row
   to fix it:

   | Row | What it does |
   | --- | --- |
   | Microphone | Shows the macOS prompt, or opens System Settings if you said no before |
   | Screen & System Audio Recording | Same; needed for Remove Computer Audio |
   | Accessibility | Needed to paste with ⌘V; without it Yapr only copies |
   | AI Gateway API Key | Asks for the key, checks it with AI Gateway, then keeps it in your Keychain |

## Using it

| You do | The bar shows |
| --- | --- |
| Press ⌥Space | Red dot and a timer (or your words, with a streaming model) |
| Press it again | "Transcribing…", then the cleaned text as it is written |
| (done) | Green dot; the text is pasted and the bar fades |
| A problem | Orange dot and the reason. If cleanup fails, the raw transcript is pasted |

The menu also has Start/Stop Dictation. While recording, the menu bar icon is filled.

## Menu

| Item | Notes |
| --- | --- |
| Transcription ▸ | AI Gateway's speech-to-text models, with prices. Default `microsoft/mai-transcribe-2` |
| Cleanup ▸ | No Cleanup, or a language model grouped by provider. Default `google/gemini-3.5-flash-lite` |
| Remove Computer Audio | On by default. Removes music and other Mac sound from the recording |
| Output ▸ | Paste into Front App (default) or Copy Only |
| Extra Instructions… | Added to the cleanup prompt, for example names or jargon |
| Change Shortcut… | Press a new combination with ⌘, ⌥ or ⌃, or a function key |
| Launch at Login | Starts Yapr when you log in |

In the model lists:

- **Live** marks streaming models, which show your words while you speak.
- **ZDR / Partial ZDR / No ZDR** shows each model's zero-data-retention support. If your AI Gateway
  team requires zero data retention, it refuses models without full ZDR; those sort last.
- Prices are AI Gateway's own: per minute of audio, or per 1M tokens (input / output). Hover a
  model for its ID, full price and description.
- Sort by Price or Name, and Refresh Model List, are at the bottom of each list. The list is
  also refreshed at launch and cached for offline use.

Settings are kept in `~/Library/Application Support/Yapr/settings.json`.

## How Remove Computer Audio works

While you dictate, Yapr also records what your Mac is playing (not its own sound) and runs both
through WebRTC's echo canceller, the one used for video calls. It measures the delay from your
speakers to your microphone in the first seconds, then subtracts the music from the microphone
signal. Measured on a 34" monitor's speakers and the MacBook microphone: about 19 dB less music
once settled, and word error rate fell from 18.9% to 10.8% on a test sentence with music
playing. It adds about 0.17 s of latency and a few ms of CPU per second of audio. Music heard
through headphones never reaches the microphone, so there is nothing to remove.

If Screen & System Audio Recording is not allowed, dictation still works with the plain
microphone and the bar says so once.

## Privacy

- In the release build, audio is sent only to AI Gateway while you dictate and is never written
  to disk. Test builds can save recordings; see Development below.
- The API key is kept in your login Keychain (`Yapr AI Gateway`) and is only sent to AI Gateway.
- Pasting puts the text on the clipboard, then sends ⌘V to the front app.
- Yapr has no local server or open port. It uses the hardened runtime, with only the audio-input
  entitlement.

## Limits

- Toggle only; there is no hold-to-talk.
- Recordings stop growing after 5 minutes.
- Paste sends the key in the ⌘V position of a US layout; on layouts such as Dvorak, use Copy Only.
- Streaming live words depend on the model: in tests `microsoft/mai-transcribe-2-streaming`
  showed words about 1 s after speech, `spacexai/grok-stt` about 2.7 s.

## Troubleshooting

- **"⌥Space is taken by another app"**: choose Change Shortcut… in the menu.
- **Text is copied but not pasted**: allow Accessibility from the Setup section.
- **"…zero data retention…" errors**: choose a model tagged ZDR, or change your team's AI
  Gateway data retention setting.
- **Music still comes through**: check Screen & System Audio Recording in Setup and that Remove
  Computer Audio is on.

## Development

The release app has no test modes. Build the test app with the Cargo `e2e` feature, run the
checks, then reinstall the release build:

```sh
scripts/build-app.sh --e2e # or E2E=1 scripts/build-app.sh
python3 e2e/check-model-prices.py
python3 e2e/dictation-flow.py
e2e/hotkey-paste.sh
scripts/build-app.sh
```

| Check | What it covers |
| --- | --- |
| `e2e/check-model-prices.py` | Every price in the menus against AI Gateway's raw pricing, and ZDR order |
| `e2e/dictation-flow.py` | Start/stop through AI Gateway with fixture audio and the real mic: cleanup, REST fallback, bad model, network failure, missing key, denied system audio. Saves `e2e/transcription-results/dictation-flow.json` |
| `e2e/hotkey-paste.sh` | Presses ⌥Space twice over a TextEdit document and checks the text is pasted. Needs Accessibility for Yapr and for your terminal. Saves `e2e/transcription-results/hotkey-paste.json` and screenshots in `e2e/screenshots/` |
| `cargo clippy --release --all-targets -- -D warnings` | Rust lint; repeat with `--features e2e` (in `app/`) |

The e2e build also takes these arguments, run from the installed binary
(`~/Applications/Yapr.app/Contents/MacOS/Yapr`) so the Keychain grant applies:

```sh
Yapr --transcribe file.wav --model microsoft/mai-transcribe-2
Yapr --stream file.wav --model microsoft/mai-transcribe-2-streaming
Yapr --cleanup "um so like…" --model google/gemini-3.5-flash-lite
Yapr --test-capture # needs existing mic and system-audio grants; no gateway calls
Yapr --echo-test e2e/fixtures/echo-music.wav e2e/fixtures/pauses.wav --model microsoft/mai-transcribe-2
Yapr --fixture e2e/fixtures/pauses.wav # the menu bar app, dictating the WAV instead of the mic
```

`YAPR_SUPPORT_DIR` points an e2e build at a different settings folder.

Test modes can write raw mic, system reference and processed WAVs under `e2e/echo-results/`,
plus transcripts and logs under `e2e/transcription-results/`. Use fixtures or consented audio;
review and delete those artifacts when done.

New signing identities do not pre-authorize `/usr/bin/codesign`. When macOS asks to use the
private key, choose **Allow**, not **Always Allow**, for each build.

## Layout

| Path | What |
| --- | --- |
| `app/src/main.rs`, `menu.rs` | App start, menu bar, dialogs and shortcut recorder |
| `app/src/dictation.rs`, `output.rs` | Start/stop flow, clipboard and paste |
| `app/src/hotkey.rs`, `settings.rs`, `models.rs` | Global shortcut, saved settings, model list and prices |
| `app/src/audio.rs`, `audio_timing.rs` | Microphone capture, resampling, stream alignment |
| `app/src/system_audio.rs`, `echo.rs` | System audio capture and echo cancellation |
| `app/src/transcribe.rs`, `transcribe_stream.rs` | AI Gateway transcription, REST and WebSocket |
| `app/src/cleanup.rs`, `gateway.rs` | Streamed cleanup and shared gateway errors |
| `app/src/key.rs`, `permissions.rs`, `overlay.rs` | Keychain key, macOS permissions, the bar |
| `scripts/` | Signing certificate and app build |
| `e2e/` | End-to-end scripts, fixtures and results |
