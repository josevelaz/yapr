# Contributing to Yapr

Thank you for your help. This guide tells you how to build Yapr, test a change, and open a pull
request. By taking part, you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md). To report
a security problem, do not open an issue: follow [SECURITY.md](SECURITY.md).

## Before you start

- For a bug, open an issue with your macOS version, the Yapr version, the models you chose, and
  the text the bar showed.
- For a large change, open an issue first so that we can agree on the approach.
- Install the tools listed in [Requirements](README.md#requirements).

## Build

1. Create the local signing certificate once. macOS ties microphone, system audio, Accessibility,
   and Keychain grants to the app's signature, and a stable certificate keeps those grants across
   rebuilds:

   ```sh
   scripts/make-signing-cert.sh
   ```

2. Build and install the release app to `~/Applications/Yapr.app`:

   ```sh
   scripts/build-app.sh
   open ~/Applications/Yapr.app
   ```

When macOS asks to let `/usr/bin/codesign` use the private key, choose **Allow**, not **Always
Allow**.

## Test

Run the checks that apply to your change. The
[Development](README.md#development) section of the README lists what each check covers and the
extra test arguments of the e2e build.

1. Lint the Rust code, in `app/`, for both builds:

   ```sh
   cargo clippy --release --all-targets -- -D warnings
   cargo clippy --release --all-targets --features e2e -- -D warnings
   ```

2. Build the test app and run the end-to-end checks. They call AI Gateway, so you need your own
   API key in Yapr. `e2e/hotkey-paste.sh` also needs Accessibility for Yapr and for your
   terminal.

   ```sh
   scripts/build-app.sh --e2e
   python3 e2e/check-model-prices.py
   python3 e2e/dictation-flow.py
   e2e/hotkey-paste.sh
   ```

3. Reinstall the release build when you are done:

   ```sh
   scripts/build-app.sh
   ```

4. If you change the release workflow scripts in `.github/scripts/release/`, run their type
   check and tests with [Bun](https://bun.sh):

   ```sh
   bun install
   bun run validate
   ```

Test modes can write audio and transcripts under `e2e/echo-results/` and
`e2e/transcription-results/`. Use the fixtures in `e2e/fixtures/` or audio that you have consent
to use. Do not commit recordings of real people, API keys, or personal paths.

## Submit a pull request

1. Fork the repository and create a branch from `main`.
2. Keep each pull request small and about one change.
3. Write commit messages as [Conventional Commits](https://www.conventionalcommits.org/), for
   example `fix(overlay): keep the bar above full-screen apps`.
4. Update `README.md` when you change what the user sees or does.
5. In the pull request description, tell what you changed, why, and which checks you ran.
6. Open the pull request against `main`. A maintainer reviews it. All changes land on `main`
   first; maintainers backport fixes to release branches (see
   [docs/releases.md](docs/releases.md)).

## License

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
