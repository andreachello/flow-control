import { dirname } from "node:path"
import { fileURLToPath } from "node:url"

// `flow serve` holds the code, the journeys and the sign-in, on a port only this machine reaches.
const engine = process.env.FLOW_ENGINE ?? "http://127.0.0.1:4001"

export default {
  turbopack: { root: dirname(fileURLToPath(import.meta.url)) },
  rewrites: async () => [
    { source: "/api/:path*", destination: `${engine}/api/:path*` },
    { source: "/auth/:path*", destination: `${engine}/auth/:path*` },
  ],
}
