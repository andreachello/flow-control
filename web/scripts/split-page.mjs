// The screens still live in ../src/serve.html, which plain `flow serve` also uses. This takes them
// apart for Next, so there is one copy while they move into React. Delete it with the last screen.
import { readFile, writeFile } from "node:fs/promises"

const page = await readFile(new URL("../../src/serve.html", import.meta.url), "utf8")
const between = (a, b) => {
  const from = page.indexOf(a)
  const to = page.lastIndexOf(b)
  if (from < 0 || to < 0) throw new Error(`serve.html has no ${a}`)
  return page.slice(from + a.length, to)
}
const css = between("<style>", "</style>")
const script = between("<script>", "</script>")
const button = /<button class="bar-s" id="soba-go" hidden>Connect compute<\/button>/
const whole = between("<body>", "</body>")
const view = '<div class="wrap" id="view"></div>'
const body = whole.slice(0, whole.indexOf("<script>")).replace(view, `${view}<div class="wrap" id="react-view" hidden></div>`)
if (!body.includes("react-view")) throw new Error("serve.html has no view to sit beside")
if (!button.test(body)) throw new Error("serve.html has no Connect compute button to replace")

await writeFile(new URL("../app/flow.generated.css", import.meta.url), css)
await writeFile(new URL("../public/flow-page.js", import.meta.url), script)
await writeFile(
  new URL("../app/body.generated.ts", import.meta.url),
  `export const BODY = ${JSON.stringify(body.replace(button, '<span id="soba-slot"></span>'))}\n`,
)
