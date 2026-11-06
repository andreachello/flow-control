import { useId } from "react"
import { iso, monday, plural } from "../lib/words"

type Week = { at: string; n: number }

export function weeks(dates: string[], n: number): Week[] {
  const end = monday(iso(new Date()))
  const out: Week[] = []
  for (let i = n - 1; i >= 0; i--) {
    const d = new Date(end)
    d.setDate(d.getDate() - i * 7)
    out.push({ at: iso(d), n: 0 })
  }
  const at = Object.fromEntries(out.map(w => [w.at, w]))
  for (const d of dates) {
    const w = at[iso(monday(d))]
    if (w) w.n++
  }
  return out
}

/** A rounded top for the scale, so the axis reads 0, 50, 100 and not 0, 47, 94. */
function niceTop(n: number) {
  const m = Math.max(n, 4)
  const p = 10 ** Math.floor(Math.log10(m))
  for (const k of [1, 1.5, 2, 3, 4, 6, 8, 10]) if (k * p >= m) return k * p
  return 10 * p
}

/** Curves between points without overshooting them, so a line never dips below zero. */
const curve = (pts: number[][]) =>
  pts.map(([x, y], i) => {
    if (!i) return `M${x} ${y}`
    const [a, b] = pts[i - 1]
    const m = (a + x) / 2
    return `C${m} ${b} ${m} ${y} ${x} ${y}`
  }).join("")

const day = (d: string) => new Date(d + "T12:00:00").toLocaleDateString("en-US", { month: "short", day: "numeric" })

export function Activity({ rows }: { rows: Week[] }) {
  const fill = useId()
  const n = rows.length
  const top = niceTop(Math.max(...rows.map(w => w.n)))
  const pts = rows.map((w, i) => [((i + 0.5) / n) * 100, 100 - (w.n / top) * 100])
  const line = curve(pts)
  const every = Math.max(1, Math.round(n / 4))
  return (
    <>
      <div className="ac" role="img" aria-label={`${rows.reduce((t, w) => t + w.n, 0)} changes over ${n} weeks`}>
        <div className="ac-y" aria-hidden="true">
          {[top, top / 2, 0].map(v => <span key={v}>{Number.isInteger(v) ? v : v.toFixed(1)}</span>)}
        </div>
        <div className="ac-p">
          <i className="ac-g" /><i className="ac-g" /><i className="ac-g" />
          <svg viewBox="0 0 100 100" preserveAspectRatio="none" aria-hidden="true">
            <defs>
              <linearGradient id={fill} x1="0" y1="0" x2="0" y2="1">
                <stop offset="0" stopColor="var(--accent)" stopOpacity=".16" />
                <stop offset="1" stopColor="var(--accent)" stopOpacity="0" />
              </linearGradient>
            </defs>
            <path d={`${line}L${pts[n - 1][0]} 100L${pts[0][0]} 100Z`} fill={`url(#${fill})`} />
            <path d={line} fill="none" stroke="var(--accent)" strokeWidth="2" vectorEffect="non-scaling-stroke" strokeLinejoin="round" />
          </svg>
          <div className="ac-h">
            {rows.map((w, i) => {
              const say = `Week of ${day(w.at)}: ${plural(w.n, "change")}`
              return <button key={w.at} style={{ "--y": `${pts[i][1]}%` } as React.CSSProperties} data-tip={say} aria-label={say} />
            })}
          </div>
        </div>
      </div>
      <div className="ac-x" style={{ "--n": n } as React.CSSProperties} aria-hidden="true">
        {rows.map((w, i) => <span key={w.at}>{i === n - 1 ? "This week" : (n - 1 - i) % every === 0 ? day(w.at) : ""}</span>)}
      </div>
    </>
  )
}
