/**
 * Forwards Yapr's AI Gateway requests for liatrio.com Google accounts. Every request must carry
 * a valid Google ID token as its bearer credential; only then is the real AI Gateway key, which
 * lives only in this deployment's environment, attached.
 */

import { createRemoteJWKSet, jwtVerify, type JWTVerifyGetKey } from "jose"

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

const googleKeys = createRemoteJWKSet(new URL("https://www.googleapis.com/oauth2/v3/certs"))

export interface RelayConfig {
  audience?: string
  gatewayKey?: string
  keys?: JWTVerifyGetKey
  fetch?: typeof fetch
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

export async function relay(request: Request, path: string, config: RelayConfig = {}): Promise<Response> {
  const audience = config.audience ?? process.env.GOOGLE_CLIENT_ID
  const gatewayKey = config.gatewayKey ?? process.env.AI_GATEWAY_KEY
  if (!audience || !gatewayKey) {
    return failure(503, "configuration_error", "The relay is not configured.")
  }
  const token = request.headers.get("authorization")?.match(/^Bearer (\S+)$/)?.[1]
  const email = token ? await verifyGoogleIdToken(token, audience, config.keys) : null
  if (!email) {
    return failure(401, "authentication_error", `Sign in with a ${DOMAIN} Google account.`)
  }

  const headers = new Headers({ authorization: `Bearer ${gatewayKey}` })
  for (const name of REQUEST_HEADERS) {
    const value = request.headers.get(name)
    if (value !== null) headers.set(name, value)
  }
  const upstream = await (config.fetch ?? fetch)(`${AI_GATEWAY}${path}`, {
    method: "POST",
    headers,
    body: await request.arrayBuffer(),
  })
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
