# Yapr liatrio.com relay

A small Vercel project that lets `liatrio.com` Google accounts use Yapr without their own AI
Gateway key. The app sends each request here with the user's Google ID token. The relay checks
the token, then forwards the request to AI Gateway with the real key. The key lives only in this
project's Vercel environment. It is never sent to the app, stored in the repository, or compiled
into a binary.

| Endpoint | Forwards to |
| --- | --- |
| `POST /api/v1/chat/completions` | AI Gateway chat completions (cleanup). The SSE stream is passed through as it arrives |
| `POST /api/v4/ai/transcription-model` | AI Gateway transcription (whole recording) |

Every request must have `Authorization: Bearer <Google ID token>`. The relay checks:

- The RS256 signature against Google's keys (`https://www.googleapis.com/oauth2/v3/certs`)
- `iss` is Google, `aud` equals `GOOGLE_CLIENT_ID`, and `exp` has not passed (60 s clock skew)
- `hd` is `liatrio.com`. If `hd` is missing, `email_verified` must be true and the email must
  end in `@liatrio.com`

If a check fails, the relay returns 401 and forwards nothing. If AI Gateway rejects the relay's
own key, the relay returns 502. The app then does not tell the user to sign in again. Other
replies from AI Gateway pass through unchanged.

## Deploy

You need the [Vercel CLI](https://vercel.com/docs/cli) and access to the Vercel team that will
own the relay.

1. Link this folder to a new Vercel project. Run this from `server/`, so `server/` becomes the
   project root:

   ```sh
   cd server
   vercel link
   ```

2. Add the environment variables for Production. When asked, mark `AI_GATEWAY_KEY` as
   sensitive:

   ```sh
   vercel env add AI_GATEWAY_KEY production
   vercel env add GOOGLE_CLIENT_ID production
   ```

   | Variable | Value |
   | --- | --- |
   | `AI_GATEWAY_KEY` | The AI Gateway API key that liatrio.com users share |
   | `GOOGLE_CLIENT_ID` | The Desktop OAuth client ID that Yapr is built with. The relay accepts only ID tokens issued for this client |

3. Deploy to production:

   ```sh
   vercel deploy --prod
   ```

4. In GitHub, add the `release` environment variable `YAPR_LIATRIO_RELAY_URL`. Set it to the
   production URL followed by `/api`, for example `https://yapr-relay.vercel.app/api`. Use the
   production domain. Preview URLs are behind Vercel Deployment Protection by default, so the
   app cannot reach them.

5. Check the deployment. A request without a token must get 401:

   ```sh
   curl -i -X POST https://yapr-relay.vercel.app/api/v1/chat/completions
   ```

If `AI_GATEWAY_KEY` or `GOOGLE_CLIENT_ID` is missing, every request gets 503.

## Limit spending

Any valid liatrio.com sign-in can call the relay, and each call spends the shared key's credits.
Before you publish a release that uses the relay, do both:

- Set a budget or spend limit on the key in AI Gateway.
- Add a rate-limit rule for `/api/*` in the Vercel Firewall for the project.

## Known limits

- **No live words.** Streaming transcription uses a WebSocket, and Vercel Functions cannot
  accept WebSocket connections. Signed-in users therefore get whole-recording transcription.
  Cleanup still streams.
- **Recording length.** A Vercel Function accepts at most 4.5 MB per request. A recording is
  sent as base64 WAV, so recordings longer than about 1 minute 50 seconds fail. The app then
  shows "The recording is too long to send." Users with their own key do not have this limit.

## Develop

```sh
cd server
bun install
bun run check # type check and tests
```
