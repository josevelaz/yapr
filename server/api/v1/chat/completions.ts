import { relay } from "../../../lib/relay.js"

export function POST(request: Request): Promise<Response> {
  return relay(request, "/v1/chat/completions")
}
