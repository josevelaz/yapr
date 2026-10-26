# Security policy

## Report a vulnerability

Do not open a public issue, discussion, or pull request for a security problem.

Report it privately through GitHub:

1. Go to the [Security tab](https://github.com/liatrio-labs/yapr/security) of this repository.
2. Choose **Report a vulnerability**.
3. Tell us what the problem is, the steps to reproduce it, the Yapr and macOS versions, and the
   effect you expect it to have.

We reply to confirm that we received the report. We work with you on a fix and on the date to
make the problem public. We give you credit in the advisory unless you ask us not to.

## Scope

Examples of problems that we want to know about:

- A way for the AI Gateway API key to leave the Keychain or go to a host other than AI Gateway.
- Audio or transcripts that the release build writes to disk or sends to a host other than AI
  Gateway.
- A way for another app or process to control Yapr or read what it records.
- Problems in the release workflow or signing that could let someone publish a changed build.

## Supported versions

We fix security problems in the latest release only.
