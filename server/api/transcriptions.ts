import { transcribe } from "../lib/relay.js"

export function POST(request: Request): Promise<Response> {
  return transcribe(request)
}
