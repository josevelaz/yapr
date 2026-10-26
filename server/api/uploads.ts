import { issueUpload } from "../lib/relay.js"

export function POST(request: Request): Promise<Response> {
  return issueUpload(request)
}
