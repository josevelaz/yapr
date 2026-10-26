# Yapr liatrio.com relay

A small Vercel project that lets `liatrio.com` Google accounts use Yapr without their own AI
Gateway key. The app sends each request here with the user's Google ID token. The relay checks
the token, then forwards the request to AI Gateway with the real key. The key lives only in this
project's Vercel environment. It is never sent to the app, stored in the repository, or compiled
into a binary.

| Endpoint | What it does |
| --- | --- |
| `POST /api/v1/chat/completions` | Forwards to AI Gateway chat completions (cleanup). The SSE stream is passed through as it arrives |
| `POST /api/uploads` | Body `{"mediaType": "audio/ogg"}` or `"audio/wav"`. Returns a random Blob pathname and a PUT URL with headers that can upload one recording to that pathname only |
| `POST /api/transcriptions` | Body `{"blob": "<pathname>", "mediaType": "..."}`. Reads the recording from Blob, sends it to AI Gateway transcription, returns the reply, and deletes the recording |
| `GET /api/cleanup` | Vercel Cron only. Deletes recordings older than 15 minutes that were uploaded but never transcribed |

Every request to the first three endpoints must have `Authorization: Bearer <Google ID token>`.
The relay checks:

- The RS256 signature against Google's keys (`https://www.googleapis.com/oauth2/v3/certs`)
- `iss` is Google, `aud` equals `GOOGLE_CLIENT_ID`, and `exp` has not passed (60 s clock skew)
- `hd` is `liatrio.com`. If `hd` is missing, `email_verified` must be true and the email must
  end in `@liatrio.com`

If a check fails, the relay returns 401 before it does anything else: it issues no upload,
reads no blob, and calls no AI Gateway endpoint. If AI Gateway rejects the relay's own key, the
relay returns 502. The app then does not tell the user to sign in again. Other replies from AI
Gateway pass through unchanged.

## How recordings travel

A Vercel Function accepts at most 4.5 MB per request body, so audio never goes to a function:

1. The app compresses the recording as Ogg Opus (24 kbit/s). Five minutes, the app's longest
   recording, is about 0.9 MB.
2. `POST /api/uploads` returns a pathname `transcriptions/<random UUID>.ogg` and a client token
   for it. The token allows one upload to that pathname, of that content type, up to 12 MiB,
   within 2 minutes. It cannot overwrite a blob.
3. The app uploads the recording straight to the private Blob store with that token.
4. `POST /api/transcriptions` sends only the pathname. The relay reads the blob, sends it to AI
   Gateway, and deletes it in all cases, also when AI Gateway fails.
5. If the upload is never followed by step 4, the daily cron job deletes it.

AI Gateway documents Ogg Opus support for its Microsoft transcription models, which include
Yapr's default `microsoft/mai-transcribe-2`. If another model rejects Ogg Opus with 400,
the app sends the same recording once more as WAV, which for five minutes is about 9.6 MB. This
also goes through Blob.

## Deploy

You need the [Vercel CLI](https://vercel.com/docs/cli) and access to the Vercel team that will
own the relay.

1. Link this folder to a new Vercel project. Run this from `server/`, so `server/` becomes the
   project root:

   ```sh
   cd server
   vercel link
   ```

2. Create a **private** Vercel Blob store and connect it to the project for Production. In the
   Vercel dashboard: open the project, then **Storage**, **Create Database**, **Blob**. Choose
   **Private** access. Connecting the store adds `BLOB_READ_WRITE_TOKEN` to the project. You do
   not set it by hand.

3. Add the other environment variables for Production. When asked, mark `AI_GATEWAY_KEY` and
   `CRON_SECRET` as sensitive:

   ```sh
   vercel env add AI_GATEWAY_KEY production
   vercel env add GOOGLE_CLIENT_ID production
   vercel env add CRON_SECRET production
   ```

   | Variable | Value |
   | --- | --- |
   | `AI_GATEWAY_KEY` | The AI Gateway API key that liatrio.com users share |
   | `GOOGLE_CLIENT_ID` | The Desktop OAuth client ID that Yapr is built with. The relay accepts only ID tokens issued for this client |
   | `CRON_SECRET` | A random string of at least 16 characters, for example from `openssl rand -hex 32`. Vercel sends it to `/api/cleanup`; other callers get 401 |
   | `BLOB_READ_WRITE_TOKEN` | Set by Vercel when you connect the Blob store in step 2 |

4. Deploy to production:

   ```sh
   vercel deploy --prod
   ```

   `vercel.json` adds the cleanup cron job. It runs once a day, which the Hobby plan allows.

5. In GitHub, add the `release` environment variable `YAPR_LIATRIO_RELAY_URL`. Set it to the
   production URL followed by `/api`, for example `https://yapr-relay.vercel.app/api`. Use the
   production domain. Preview URLs are behind Vercel Deployment Protection by default, so the
   app cannot reach them.

6. Check the deployment. A request without a token must get 401:

   ```sh
   curl -i -X POST https://yapr-relay.vercel.app/api/uploads
   ```

If `AI_GATEWAY_KEY` or `GOOGLE_CLIENT_ID` is missing, every request gets 503. If
`BLOB_READ_WRITE_TOKEN` is missing, uploads and transcriptions get 503.

## Limit spending

Any valid liatrio.com sign-in can call the relay, and each call spends the shared key's credits
and some Blob storage and transfer. Before you publish a release that uses the relay, do both:

- Set a budget or spend limit on the key in AI Gateway.
- Add a rate-limit rule for `/api/*` in the Vercel Firewall for the project.

## Known limits

- **No live words.** Streaming transcription uses a WebSocket, and Vercel Functions cannot
  accept WebSocket connections. Signed-in users therefore get whole-recording transcription.
  Cleanup still streams.
- Recording length is not limited by the relay. The 4.5 MB request limit does not apply,
  because the audio goes to Blob and not to a function.

## Develop

```sh
cd server
bun install
bun run check # type check and tests
```
