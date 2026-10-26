import { vercelBlobStore } from "../lib/blob.js"

const STALE_MS = 15 * 60 * 1000

/** Vercel Cron: deletes recordings whose transcription request never arrived. */
export async function GET(request: Request): Promise<Response> {
  const secret = process.env.CRON_SECRET
  if (!secret || request.headers.get("authorization") !== `Bearer ${secret}`) {
    return new Response(null, { status: 401 })
  }
  const store = vercelBlobStore()
  if (!store) return new Response(null, { status: 503 })
  return Response.json({ removed: await store.removeOlderThan(Date.now() - STALE_MS) })
}
