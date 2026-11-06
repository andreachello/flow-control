"use client"

import { useEffect, useMemo, useRef, useState } from "react"
import { Badge, tint } from "../lib/badge"
import { api, get, keptChanges, known, link, pickedProduct, readChanges, SignedOut, type Commit, type Journey, type Page, type Summary } from "../lib/project"
import { useRun } from "../lib/run"
import { ago, iso, monday, plural, when } from "../lib/words"
import { Activity, weeks } from "./activity"
import { RunControl } from "./run-control"

/** Enough history to say which journeys change most, without reading all of a long project. */
const LOAD_MOST = 120
/** How long what the tab knows counts as current. */
const FRESH_FOR = 30000
/** How many rows each list here shows. The rest are a link away. */
const SHOWN = 6

function Skeleton() {
  return (
    <div className="ex-cards on">
      <div className="page-h"><span className="sk" style={{ width: 160, height: 22 }} /></div>
      <div className="stats">
        {[1, 2, 3, 4].map(i => (
          <div className="sk-box" style={{ margin: 0 }} key={i}>
            <span className="sk" style={{ width: "40%", height: 20 }} /><span className="sk" style={{ width: "60%" }} />
          </div>
        ))}
      </div>
      <div className="sk-box" style={{ "--h": "220px", marginTop: 12 } as React.CSSProperties}><span className="sk" style={{ width: "24%" }} /></div>
    </div>
  )
}

function Figure({ n, label }: { n: number | string; label: string }) {
  return <div className="stat"><b>{n}</b><span>{label}</span></div>
}

function Latest({ c, project, book, notes }: { c: Commit; project: string; book: Journey[]; notes: number[] }) {
  return (
    <li>
      <a href={link(project, "change", c.id)}>
        <b className={c.quiet ? "q" : ""}>{c.title}</b>
        {book.length > 0 && <span className="ic">{notes.map(g => <Badge key={g} g={g} book={book} />)}</span>}
        <i className="who" title={c.author}>{(c.author || "?").charAt(0).toUpperCase()}</i>
        <small className="dt">{ago(c.date)}</small>
      </a>
    </li>
  )
}

export function Overview({ project, soba, signedOut }: { project: string; soba: boolean; signedOut: () => void }) {
  // Drawn from what the tab already knows, then checked. A screen that waits is a screen that blinks.
  const [had] = useState(() => known(project))
  const [list, setList] = useState<Commit[] | null>(had?.list ?? null)
  const [total, setTotal] = useState(had?.total ?? 0)
  const [book, setBook] = useState<Journey[]>(had?.book ?? [])
  const [activity, setActivity] = useState<[string, string][] | null>(had?.act ?? null)
  const [requests, setRequests] = useState<unknown[] | null>(had?.cat ?? null)
  const [read, setRead] = useState<Record<string, Summary>>(() => (had ? keptChanges(project, had.stamp) : {}))
  const [problem, setProblem] = useState<string | null>(null)
  const [loaded, setLoaded] = useState(0)
  const reload = useRef(() => setLoaded(n => n + 1))
  const { run, start } = useRun(project, reload.current)

  useEffect(() => {
    let live = true
    const on = () => live
    ;(async () => {
      try {
        if (had?.act && loaded === 0 && Date.now() - had.at < FRESH_FOR) return await readChanges(project, read, had.stamp, had.list, setRead, on)
        const all: Commit[] = []
        let first: Page | null = null
        for (;;) {
          const d = await get<Page>(api(project, "commits", `?skip=${all.length}`))
          first ??= d
          all.push(...d.commits)
          if (!live) return
          if (d.commits.length !== d.size || all.length >= LOAD_MOST) break
        }
        const seen = await get<[string, string][]>(api(project, "activity"))
        if (!live) return
        setList(all)
        setTotal(first.project.changes)
        setBook(first.journeys)
        setActivity(seen)
        get<unknown[]>(api(project, "requests")).then(r => live && setRequests(r), () => {})
        await readChanges(project, {}, first.stamp, all, setRead, on)
      } catch (e) {
        if (!live) return
        if (e instanceof SignedOut) signedOut()
        else setProblem(e instanceof Error ? e.message : String(e))
      }
    })()
    return () => {
      live = false
    }
  }, [project, loaded, signedOut])

  const noted = (c: Commit) => (read[c.id] && !c.quiet ? read[c.id].focus : [])
  const done = useMemo(() => (list ?? []).filter(c => read[c.id]), [list, read])

  if (problem) {
    return (
      <div className="pj-empty">
        <b>That did not load</b><p>{problem}</p>
        <p><button className="bar-s" onClick={() => { setProblem(null); setLoaded(n => n + 1) }}>Try again</button></p>
      </div>
    )
  }
  if (!list || !activity) return <Skeleton />

  const oldest = activity.length ? activity[activity.length - 1][0] : iso(new Date())
  const span = Math.ceil((monday(iso(new Date())).getTime() - monday(oldest).getTime()) / 6048e5) + 1
  const shown = Math.max(8, Math.min(26, span))
  const rows = weeks(activity.map(a => a[0]), shown)
  const people = new Set(activity.map(a => a[1])).size
  const product = pickedProduct(project, book)
  const mine = book.map((_, g) => g).filter(g => !product || book[g].product === product)
  const latest = list.filter(c => !product || !read[c.id] || noted(c).some(g => mine.includes(g)))
  const counts = mine.map(g => ({ g, n: done.filter(c => noted(c).includes(g)).length })).sort((a, b) => b.n - a.n || a.g - b.g)
  const top = Math.max(1, ...counts.map(x => x.n))
  const reading = done.length < list.length
  const last = list[0] ? ago(list[0].date) : ""
  const lastSay = last === "Today" || last === "Yesterday" ? last.toLowerCase() : list[0] ? when(list[0].date) : ""

  return (
    <div className="ex-cards on">
      <div className="page-h"><h2>{product || project}</h2><span className="ghl-n">{list[0] ? `Last change ${lastSay}` : ""}</span></div>
      <div className="stats">
        <Figure n={total} label="Changes" />
        <Figure n={rows[rows.length - 1].n} label="This week" />
        <Figure n={mine.length || (requests ? requests.length : "…")} label={book.length ? "Journeys" : "Requests"} />
        <Figure n={people} label="People" />
      </div>
      <div className="cols">
        <div>
          <div className="pn">
            <div className="pn-h"><b>Activity</b><small>Last {shown} weeks</small></div>
            <div className="pn-b"><Activity rows={rows} /></div>
          </div>
          <div className="pn">
            <div className="pn-h"><b>Latest changes</b><a href={link(project, "changes")}>All changes</a></div>
            <ul className="fd">{latest.slice(0, SHOWN).map(c => <Latest key={c.id} c={c} project={project} book={book} notes={noted(c)} />)}</ul>
          </div>
        </div>
        <div className="pn">
          <div className="pn-h"><b>Most changed journeys</b></div>
          {book.length ? (
            <>
              <div className="pn-b">
                <ul className="rank">
                  {!done.length
                    ? mine.slice(0, SHOWN).map(i => <li className="rank-sk" key={i}><a><i className="sk" /><span className="sk" /><small /><u /></a></li>)
                    : counts.slice(0, SHOWN).map(({ g, n }) => (
                        <li key={g}>
                          <a href={link(project, "journey", String(g))}>
                            <Badge g={g} book={book} /><span>{book[g].title}</span><small>{n}</small>
                            <u style={{ "--c": tint(g, book) } as React.CSSProperties}><i style={{ width: `${(n / top) * 100}%` }} /></u>
                          </a>
                        </li>
                      ))}
                </ul>
              </div>
              <div className="pn-e pn-f">
                <span>{reading ? `Reading ${done.length} of ${list.length} changes` : `From the latest ${plural(done.length, "change")}`}</span>
                <a href={link(project, "journeys")}>View all journeys</a>
              </div>
            </>
          ) : (
            <RunControl task="journeys" lead="Group this project by what people come to do." run={run} start={start} soba={soba} />
          )}
        </div>
      </div>
    </div>
  )
}
