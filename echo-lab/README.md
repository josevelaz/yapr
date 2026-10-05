# Echo lab (macOS-only prototype)

This standalone crate uses ScreenCaptureKit 11.0.0 and bundled WebRTC audio
processing 2.1.0. It does not change the dictation helper. All WAV artifacts are
16 kHz mono **32-bit float**, not PCM16. No cloud calls or commits.

## Build and repeat the experiments

```sh
brew install meson ninja
cargo build --release
mkdir -p out
timeout 90s target/release/offline | tee out/offline.log
timeout 60s python3 live-harness.py 22
cargo fmt --check
cargo clippy --release --all-targets -- -D warnings
```

The first build needs network access for Cargo and Meson's Abseil source fetch.
This Mac already had pkg-config, Apple Command Line Tools, and Rust's
`rust-objcopy` (Homebrew Rust 1.99.0). Meson 1.12.1 and Ninja 1.13.2 were installed
with Homebrew. There was no need for a system WebRTC library, or an AEC fallback.
`build.rs` adds `/usr/lib/swift` to the executable's rpath: without it the
ScreenCaptureKit Swift bridge builds but fails at launch with
`@rpath/libswift_Concurrency.dylib` missing.

`offline` defaults to `../e2e/fixtures/pauses.wav` (22.05 kHz mono). Pass another
fixture path as its first argument. It generates a seeded chord progression
with drums, and a seeded voiced-vowel/breath interference signal. The latter
is **speech-like synthesis**, not intelligible TTS or a `say` recording. It tests
two far-end spectral patterns without depending on installed voices or audio
hardware. The room has a 50 ms direct path and decaying reflections at 63, 79,
and 97 ms. Echo RMS is 6 dB above target speech RMS in active speech windows;
light seeded noise is added without clipping. Each run lasts 22.877 s, including
6 s of initial echo-only audio and a 4 s tail.

`offline` writes `{music,speech-like,music-adaptive,speech-like-adaptive}-`
`{target,ref,echo,mic,clean}.wav`, a speech-only control, and `play-music.wav`.
It exits nonzero if any ERLE is below 15 dB, sample counts differ, output is not
finite, or the timestamp/drift/gap checks fail. Arbitrary input chunk sizes and
the final partial 10 ms frame exercise buffering and flush. Timestamp checks
use irregular packets with -100/+150 ppm drift and an explicit missing packet.

## Measurement results

See `out/offline.log`, `out/live.log`, and the WAVs for the recorded run.
ERLE uses known echo power divided by **total output power** in echo-only
regions: it includes noise, so it is a conservative residual measure. It skips
the first 3 s of adaptation and excludes 200 ms around each active speech
window, leaving 8.83 s of echo-only measurements. Speech windows use 10 ms
clean-fixture RMS > 0.003 before normalization. Target active RMS is 0.045.

Speech correlation and unscaled SNR compare output with clean target, finding
one global lag in 0..100 ms. No output gain fitting is applied to SNR; the
least-squares speech gain is printed separately to reveal suppression. Lag is
14 ms in these runs. Speech-region RMS uses the original active-window mask.

| Interference / delay | ERLE dB | Correlation before → after | SNR before → after dB | Echo-only RMS mic → clean | Speech RMS mic → clean |
|---|---:|---|---|---|---|
| Music / 50 ms external | 20.46 | .4469 → .8070 | -6.00 → 4.39 | .094061 → .008922 | .100368 → .041925 |
| Speech-like / 50 ms external | 24.17 | .4517 → .8062 | -6.00 → 4.32 | .085933 → .005320 | .100636 → .042609 |
| Music / automatic | 23.23 | .4469 → .5259 | -6.00 → 1.41 | .094061 → .006486 | .100368 → .023541 |
| Speech-like / automatic | 29.07 | .4517 → .7791 | -6.00 → 4.02 | .085933 → .003025 | .100636 → .037642 |

The speech-only control gets correlation .8739, SNR 6.22 dB and speech gain
.8083: high-pass filtering and NS themselves change the waveform. Calibrated
mixed runs retain speech gains .7520/.7638. **Automatic-delay music still
suppresses speech too much** (gain .2753). High echo-only ERLE alone does not
prove useful dictation. Transcription accuracy needs the lead's separate check.

CPU is current-thread CPU time around the AEC calls, not wall time and not
resampling/capture/disk I/O. Calibrated runs cost about 1.4 ms per audio-second;
automatic runs cost about 14–16 ms per audio-second (1.4–1.6% of one M5 Pro core).
See the logs for each run's measured values.

### Configuration investigation

WebRTC defaults achieved ~30 dB echo reduction but poor mixed-music speech
correlation (~.29). Disabling NS did not solve it. The final configuration uses
moderate NS, a high-pass filter, no AGC, and NS noise analysis on the exported
linear AEC output. It increases the ERLE estimator caps to 100 (20 dB) and uses
gentler residual masks: echo/near-end transparency 5, suppression 6, and
echo/microphone transparency .5 in both bands/modes. These are experimental
settings validated in the synthetic room, not production defaults.

`EchoCanceller::new()` uses automatic delay estimation. `with_delay(Some(ms))`
also enables WebRTC's **external delay estimator**; this is more than just an
initial hint. The 50 ms experiment uses known simulated room delay, not a delay
silently optimized from the clean target. Do not pin a real-room value without
measuring it. Earlier diagnostic configurations and outputs are in
`ablation-*.log`; only the final `out/*.wav` represent delivered settings.

### Live run

`live SECONDS [--request-permission] [--delay-ms MS]` captures the default mic
with CPAL and system output with ScreenCaptureKit. The harness uses `timeout`,
waits for `READY`, starts loud synthetic music with `afplay`, starts
`pauses.wav` with a second `afplay` after 6 s, and stops child players on exit.
It does not change system volume. The live WAVs are `live-mic.wav`,
`live-ref.wav`, and `live-clean.wav`.

Permissions were already granted on this host; **no permission blocker**.
The successful 22 s automatic-delay run used MacBook Pro Microphone at 48 kHz
mono and Sceptre O34 output. It measured mic RMS .019577 (-34.17 dBFS),
reference .180987 (-14.85 dBFS), cleaned .002329 (-52.66 dBFS), and 18.70 dB
reduction after 3 s warmup. AEC CPU was 19.71 ms/audio-second. Both callback-drop
counts and both zero-fill counts were zero.

This is **apparent ERLE**, not an isolated near-end speech test: both afplay
signals are in the far-end reference and should be canceled. Ambient sounds
may be present. It proves real simultaneous capture and cancellation, not that
the user's dictated words survive.

The first live attempts showed ~56–58% missing reference samples despite no
dropped callbacks. SCK was delivering 20 ms audio packets in bursts up to
~105–139 ms late. Increasing the worker jitter allowance from 40 to 150 ms removed
all reference underflows and improved live reduction from ~11–13 to 18.70–21.45 dB.
This is why a simple pair-of-FIFOs scheme with a 10–40 ms wait is not enough.

## Integration and threading

- Move `EchoCanceller` to `app/src/echo.rs`, and the tap to
  `app/src/system_audio.rs`; share the small decimator/timeline routines.
  No edits to those paths were made here.
- Recorder starts SCK and the mic, then runs one worker that owns WebRTC.
  SCK retains a CMSampleBuffer and uses `try_send` to a bounded raw channel.
  Its decoder worker reads float stereo (planar or interleaved), averages to
  mono, and FIR-decimates 48→16 kHz. Mic callbacks copy interleaved samples to
  another bounded channel; the recorder worker downmixes/decimates them.
  There is no callback DSP, file I/O, blocking send, or shared canceller lock.
  The mic copy allocates; this prototype is not allocation-free hard real-time.
- Keep PTS/capture timestamps on the shared mach-uptime clock, including FIR
  group delay. Sample each timeline onto the same 16 kHz grid, then call render
  analysis before capture processing for every 160-sample frame. Neighboring
  timestamps determine actual packet rates, so clock drift does not accumulate
  as FIFO delay. Interpolation crosses packet boundaries. Bounded queues and
  timestamped zero-fill preserve dropped intervals rather than shortening time.
- Wait off callbacks for the 150 ms reference jitter allowance; do not slide
  reference forward to cancel acoustic path delay. Let AEC estimate that delay,
  or supply a measured external value. The current live allowance contributes
  160 ms including one full frame; AEC/NS adds the measured 14 ms and FIR adds
  ~0.65 ms (timestamps compensate it). Budget **about 175 ms** added latency,
  apart from stream startup. Tune the allowance from observed callback ages,
  not from a guess. AEC also needs initial adaptation time.
- Permission preflight fails immediately with a clear Screen & System Audio
  Recording error. Only explicit `request_permission()` asks macOS; it returns
  within 2 s even if a human has not answered. SCK v11's own synchronous calls
  have a built-in 30 s deadline; the harness has a process deadline as well.
  Recorder should catch tap errors and record **plain mic**, without repeatedly
  prompting or sending silent AEC output. Show the user System Settings >
  Privacy & Security > Screen & System Audio Recording, identify the launching
  app/terminal or binary, and ask them to grant access and relaunch. Microphone
  permission is separate. Do not reset TCC to test denial.
- Add a stream-error fallback in the helper for revoked permission/device
  changes. This tap surfaces decode errors on its channel; `live` fails on no
  callbacks after 3 s and records gap/drop counters. It is not a full device
  reconnection service. No process-tap fallback was needed.

Remaining work: measure real near-end speech with live music, calibrate delay
without clean target access, and check transcription quality. The synthetic
speech-like source does not cover intelligible far-end speech, stereo speakers
with different acoustic paths, moving users, clipping, or output-volume changes.
