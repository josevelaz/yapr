/**
 * Forwards Yapr's AI Gateway requests for liatrio.com Google accounts. Every request must carry
 * a valid Google ID token as its bearer credential; only then is the real AI Gateway key, which
 * lives only in this deployment's environment, attached.
 */

import { createRemoteJWKSet, jwtVerify, type JWTVerifyGetKey } from "jose"

import { PREFIX, vercelBlobStore, type AudioStore } from "./blob.js"

export const DOMAIN = "liatrio.com"

const AI_GATEWAY = "https://ai-gateway.vercel.sh"
const ISSUERS = ["https://accounts.google.com", "accounts.google.com"]
const REQUEST_HEADERS = [
  "content-type",
  "ai-gateway-protocol-version",
  "ai-transcription-model-specification-version",
  "ai-model-id",
]
const RESPONSE_HEADERS = ["content-type", "cache-control"]
const AUDIO_TYPES: Record<string, string> = { "audio/ogg": "ogg", "audio/wav": "wav" }
// A 5-minute recording is about 0.9 MB as Ogg/Opus and 9.6 MB as the WAV fallback.
const MAX_AUDIO_BYTES = 12 * 1024 * 1024
const UPLOAD_SECONDS = 120
const BLOB_PATH = new RegExp(`^${PREFIX}[0-9a-f-]{36}\\.(ogg|wav)$`)

const googleKeys = createRemoteJWKSet(new URL("https://www.googleapis.com/oauth2/v3/certs"))

export interface RelayConfig {
  audience?: string
  gatewayKey?: string
  keys?: JWTVerifyGetKey
  fetch?: typeof fetch
  store?: AudioStore | null
}

/** Returns the account email, or null unless the token is valid for `audience` and DOMAIN. */
export async function verifyGoogleIdToken(
  token: string,
  audience: string,
  keys: JWTVerifyGetKey = googleKeys,
): Promise<string | null> {
  try {
    const { payload } = await jwtVerify(token, keys, {
      issuer: ISSUERS,
      audience,
      algorithms: ["RS256"],
      requiredClaims: ["exp", "email"],
      clockTolerance: 60,
    })
    const email = typeof payload.email === "string" ? payload.email.toLowerCase() : ""
    const inDomain =
      typeof payload.hd === "string"
        ? payload.hd.toLowerCase() === DOMAIN
        : payload.email_verified === true && email.endsWith(`@${DOMAIN}`)
    return inDomain && email ? email : null
  } catch {
    return null
  }
}

function failure(status: number, type: string, message: string): Response {
  return Response.json({ error: { type, message } }, { status })
}

/** Checks configuration and the caller's token; returns the response to send when either fails. */
async function deny(request: Request, config: RelayConfig, needsStore: boolean): Promise<Response | null> {
  const audience = config.audience ?? process.env.GOOGLE_CLIENT_ID
  const gatewayKey = config.gatewayKey ?? process.env.AI_GATEWAY_KEY
  if (!audience || !gatewayKey || (needsStore && !store(config))) {
    return failure(503, "configuration_error", "The relay is not configured.")
  }
  const token = request.headers.get("authorization")?.match(/^Bearer (\S+)$/)?.[1]
  const email = token ? await verifyGoogleIdToken(token, audience, config.keys) : null
  return email ? null : failure(401, "authentication_error", `Sign in with a ${DOMAIN} Google account.`)
}

function store(config: RelayConfig): AudioStore | null {
  return config.store === undefined ? vercelBlobStore() : config.store
}

async function forward(request: Request, path: string, body: BodyInit, config: RelayConfig): Promise<Response> {
  const headers = new Headers({ authorization: `Bearer ${config.gatewayKey ?? process.env.AI_GATEWAY_KEY}` })
  for (const name of REQUEST_HEADERS) {
    const value = request.headers.get(name)
    if (value !== null) headers.set(name, value)
  }
  const upstream = await (config.fetch ?? fetch)(`${AI_GATEWAY}${path}`, { method: "POST", headers, body })
  // A 401 from the relay means "sign in again" to the app, so the relay's own bad key must differ.
  if (upstream.status === 401 || upstream.status === 403) {
    return failure(502, "relay_key_error", "AI Gateway rejected the relay's key. Tell the relay owner.")
  }
  const responseHeaders = new Headers()
  for (const name of RESPONSE_HEADERS) {
    const value = upstream.headers.get(name)
    if (value !== null) responseHeaders.set(name, value)
  }
  return new Response(upstream.body, { status: upstream.status, headers: responseHeaders })
}

/** Passes a request body through to AI Gateway and streams the reply back. */
export async function relay(request: Request, path: string, config: RelayConfig = {}): Promise<Response> {
  return (await deny(request, config, false)) ?? forward(request, path, await request.arrayBuffer(), config)
}

/** Issues a short-lived upload for one recording at a new random pathname. */
export async function issueUpload(request: Request, config: RelayConfig = {}): Promise<Response> {
  const denied = await deny(request, config, true)
  if (denied) return denied
  const { mediaType } = (await request.json().catch(() => ({}))) as { mediaType?: unknown }
  const extension = typeof mediaType === "string" ? AUDIO_TYPES[mediaType] : undefined
  if (!extension) return failure(400, "invalid_request_error", "Unsupported recording format.")
  const pathname = `${PREFIX}${crypto.randomUUID()}.${extension}`
  const validUntil = Date.now() + UPLOAD_SECONDS * 1000
  const ticket = await store(config)!.ticket(pathname, mediaType as string, MAX_AUDIO_BYTES, validUntil)
  return Response.json({ pathname, ...ticket })
}

/** Reads an uploaded recording, sends it to AI Gateway transcription, and always deletes it. */
export async function transcribe(request: Request, config: RelayConfig = {}): Promise<Response> {
  const denied = await deny(request, config, true)
  if (denied) return denied
  const { blob, mediaType } = (await request.json().catch(() => ({}))) as { blob?: unknown; mediaType?: unknown }
  if (typeof blob !== "string" || !BLOB_PATH.test(blob) || AUDIO_TYPES[String(mediaType)] !== blob.split(".")[1]) {
    return failure(400, "invalid_request_error", "Unknown recording upload.")
  }
  const audioStore = store(config)!
  try {
    const audio = await audioStore.read(blob)
    if (!audio) return failure(404, "invalid_request_error", "The recording upload was not found.")
    if (audio.byteLength > MAX_AUDIO_BYTES) return failure(413, "invalid_request_error", "The recording is too large.")
    const body = JSON.stringify({ audio: Buffer.from(audio).toString("base64"), mediaType })
    const reply = await forward(request, "/v4/ai/transcription-model", body, config)
    return new Response(await reply.arrayBuffer(), { status: reply.status, headers: reply.headers })
  } finally {
    await audioStore.remove(blob).catch(() => {})
  }
}
