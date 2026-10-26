import { beforeAll, describe, expect, test } from "bun:test"
import { createLocalJWKSet, exportJWK, generateKeyPair, SignJWT, type JWTPayload, type JWTVerifyGetKey } from "jose"

import { relay, type RelayConfig } from "../lib/relay.js"

const AUDIENCE = "client.apps.googleusercontent.com"
const GATEWAY_KEY = "test-gateway-key"

let privateKey: CryptoKey
let keys: JWTVerifyGetKey

beforeAll(async () => {
  const pair = await generateKeyPair("RS256")
  privateKey = pair.privateKey
  keys = createLocalJWKSet({ keys: [{ ...(await exportJWK(pair.publicKey)), kid: "k1", alg: "RS256" }] })
})

function idToken(claims: JWTPayload = {}, expiresIn: string | number = "1h"): Promise<string> {
  return new SignJWT({ hd: "liatrio.com", email: "dev@liatrio.com", email_verified: true, ...claims })
    .setProtectedHeader({ alg: "RS256", kid: "k1" })
    .setIssuer("https://accounts.google.com")
    .setAudience(AUDIENCE)
    .setIssuedAt()
    .setExpirationTime(expiresIn)
    .sign(privateKey)
}

interface Sent {
  url: string
  headers: Headers
  body: string
}

async function call(token: string | null, config: Partial<RelayConfig> = {}) {
  const sent: Sent[] = []
  const upstream = (async (url: string, init: RequestInit) => {
    sent.push({
      url,
      headers: new Headers(init.headers),
      body: new TextDecoder().decode(init.body as ArrayBuffer),
    })
    return new Response("data: {\"x\":1}\n\ndata: [DONE]\n\n", {
      status: 200,
      headers: { "content-type": "text/event-stream", "content-encoding": "gzip", "set-cookie": "a=b" },
    })
  }) as unknown as typeof fetch
  const headers: Record<string, string> = {
    "content-type": "application/json",
    "ai-model-id": "microsoft/mai-transcribe-2",
    cookie: "session=1",
  }
  if (token !== null) headers.authorization = `Bearer ${token}`
  const request = new Request("https://relay.test/api/v1/chat/completions", {
    method: "POST",
    headers,
    body: "{\"model\":\"m\"}",
  })
  const response = await relay(request, "/v1/chat/completions", {
    audience: AUDIENCE,
    gatewayKey: GATEWAY_KEY,
    keys,
    fetch: upstream,
    ...config,
  })
  return { response, sent }
}

describe("relay", () => {
  test("forwards a liatrio.com request with the gateway key and streams the reply back", async () => {
    const { response, sent } = await call(await idToken())
    expect(response.status).toBe(200)
    expect(await response.text()).toBe("data: {\"x\":1}\n\ndata: [DONE]\n\n")
    expect(response.headers.get("content-type")).toBe("text/event-stream")
    expect(response.headers.get("content-encoding")).toBeNull()
    expect(response.headers.get("set-cookie")).toBeNull()
    expect(sent).toHaveLength(1)
    expect(sent[0]!.url).toBe("https://ai-gateway.vercel.sh/v1/chat/completions")
    expect(sent[0]!.headers.get("authorization")).toBe(`Bearer ${GATEWAY_KEY}`)
    expect(sent[0]!.headers.get("ai-model-id")).toBe("microsoft/mai-transcribe-2")
    expect(sent[0]!.headers.get("cookie")).toBeNull()
    expect(sent[0]!.body).toBe("{\"model\":\"m\"}")
  })

  test("accepts a verified liatrio.com email when hd is absent", async () => {
    const { response } = await call(await idToken({ hd: undefined }))
    expect(response.status).toBe(200)
  })

  const rejected: [string, () => Promise<string | null>][] = [
    ["a missing token", async () => null],
    ["a malformed token", async () => "not-a-jwt"],
    ["another hosted domain", () => idToken({ hd: "example.com" })],
    ["an unverified email without hd", () => idToken({ hd: undefined, email_verified: false })],
    ["a lookalike email domain", () => idToken({ hd: undefined, email: "dev@notliatrio.com" })],
    ["an expired token", () => idToken({}, Math.floor(Date.now() / 1000) - 120)],
    ["another audience", () => new SignJWT({ hd: "liatrio.com", email: "dev@liatrio.com" })
      .setProtectedHeader({ alg: "RS256", kid: "k1" })
      .setIssuer("https://accounts.google.com")
      .setAudience("other.apps.googleusercontent.com")
      .setExpirationTime("1h")
      .sign(privateKey)],
    ["another issuer", () => new SignJWT({ hd: "liatrio.com", email: "dev@liatrio.com" })
      .setProtectedHeader({ alg: "RS256", kid: "k1" })
      .setIssuer("https://evil.example")
      .setAudience(AUDIENCE)
      .setExpirationTime("1h")
      .sign(privateKey)],
    ["a forged signature", async () => {
      const [header, payload] = (await idToken()).split(".")
      return `${header}.${payload}.${Buffer.from("x".repeat(256)).toString("base64url")}`
    }],
  ]
  for (const [name, token] of rejected) {
    test(`returns 401 without forwarding for ${name}`, async () => {
      const { response, sent } = await call(await token())
      expect(response.status).toBe(401)
      expect(sent).toHaveLength(0)
      const body = (await response.json()) as { error: { type: string } }
      expect(body.error.type).toBe("authentication_error")
    })
  }

  test("reports a rejected gateway key as 502, not as a sign-in failure", async () => {
    const rejecting = (async () =>
      Response.json({ error: { message: "Invalid API key" } }, { status: 401 })) as unknown as typeof fetch
    const { response } = await call(await idToken(), { fetch: rejecting })
    expect(response.status).toBe(502)
  })

  test("passes other upstream errors through", async () => {
    const limited = (async () =>
      Response.json({ error: { message: "slow down" } }, { status: 429 })) as unknown as typeof fetch
    const { response } = await call(await idToken(), { fetch: limited })
    expect(response.status).toBe(429)
    expect(await response.json()).toEqual({ error: { message: "slow down" } })
  })

  test("returns 503 without forwarding when the gateway key is not set", async () => {
    const { response, sent } = await call(await idToken(), { gatewayKey: "" })
    expect(response.status).toBe(503)
    expect(sent).toHaveLength(0)
  })
})
