/**
 * Recordings travel through a private Vercel Blob store so that audio is never a function
 * request body. The app uploads with a one-pathname client token; the relay reads and deletes.
 */

import { del, get, list } from "@vercel/blob"
import { generateClientTokenFromReadWriteToken } from "@vercel/blob/client"

export const PREFIX = "transcriptions/"

// The app replays these as a plain PUT, so they must match what @vercel/blob's client put() sends.
const BLOB_API = "https://vercel.com/api/blob"
const BLOB_API_VERSION = "13"

export interface UploadTicket {
  url: string
  headers: Record<string, string>
}

export interface AudioStore {
  ticket(pathname: string, contentType: string, maximumSizeInBytes: number, validUntil: number): Promise<UploadTicket>
  read(pathname: string): Promise<Uint8Array | null>
  remove(pathname: string): Promise<void>
  removeOlderThan(time: number): Promise<number>
}

export function vercelBlobStore(token = process.env.BLOB_READ_WRITE_TOKEN): AudioStore | null {
  if (!token) return null
  const storeId = token.split("_")[3] ?? ""
  return {
    async ticket(pathname, contentType, maximumSizeInBytes, validUntil) {
      const clientToken = await generateClientTokenFromReadWriteToken({
        token,
        pathname,
        allowedContentTypes: [contentType],
        maximumSizeInBytes,
        validUntil,
        addRandomSuffix: false,
        allowOverwrite: false,
      })
      return {
        url: `${BLOB_API}/?${new URLSearchParams({ pathname })}`,
        headers: {
          authorization: `Bearer ${clientToken}`,
          "x-api-version": BLOB_API_VERSION,
          "x-vercel-blob-access": "private",
          "x-vercel-blob-store-id": storeId,
          "x-content-type": contentType,
          "x-add-random-suffix": "0",
          "x-allow-overwrite": "0",
        },
      }
    },
    async read(pathname) {
      const result = await get(pathname, { access: "private", token, useCache: false })
      if (result?.statusCode !== 200) return null
      return new Uint8Array(await new Response(result.stream).arrayBuffer())
    },
    async remove(pathname) {
      await del(pathname, { token })
    },
    async removeOlderThan(time) {
      let removed = 0
      let cursor: string | undefined
      do {
        const page = await list({ prefix: PREFIX, cursor, token })
        const stale = page.blobs.filter((blob) => blob.uploadedAt.getTime() < time).map((blob) => blob.url)
        if (stale.length > 0) await del(stale, { token })
        removed += stale.length
        cursor = page.hasMore ? page.cursor : undefined
      } while (cursor)
      return removed
    },
  }
}
