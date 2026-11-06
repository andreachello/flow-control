// Flow is Rust, and Soba's SDK is not, so this does the SDK's half: one request on stdin, one JSON
// line on stdout. SOBA_KEY comes from the environment flow gave it, and is never printed.
import { Soba, SobaError } from "@soba-so/sdk"
import { sobaUserId } from "@soba-so/sdk/identity"

const said = await new Promise(done => {
  let text = ""
  process.stdin.setEncoding("utf8").on("data", s => (text += s)).on("end", () => done(text))
})
const ask = JSON.parse(said)
const user = sobaUserId(ask.login)

// The SDK has no call for this, so it is the documented request, with its status and body kept.
async function session() {
  const web = process.env.SOBA_WEB_URL ?? "https://soba.so/v1"
  const r = await fetch(`${web}/end_users/session`, {
    method: "POST",
    headers: { Authorization: `Bearer ${process.env.SOBA_KEY}`, "Content-Type": "application/json" },
    body: JSON.stringify({ user }),
  })
  return { status: r.status, body: await r.text() }
}

// Sealed to the person's own machines by default, so Soba routes the run and cannot read it.
// `fallback: false` because a prompt holds a project's code, and goes nowhere else if Soba is down.
async function run() {
  const soba = new Soba({ fallback: false, encrypt: process.env.FLOW_SOBA_ENCRYPT !== "off" })
  const going = await soba.run({ user, model: "auto", prompt: ask.prompt })
  return { text: await going.text() }
}

let out
try {
  out = ask.op === "session" ? await session() : await run()
} catch (e) {
  out = e instanceof SobaError ? { refused: { code: e.code, message: e.message } } : { failed: String(e?.message ?? e) }
}
process.stdout.write(JSON.stringify(out) + "\n", () => process.exit(0))
