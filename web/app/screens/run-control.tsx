"use client"

import { ConnectNudge, RunError, RunLimit } from "@soba-so/react"
import { asError, type Run, type Task } from "../lib/run"

const TASKS: Record<Task, { go: string; doing: string }> = {
  journeys: { go: "Find journeys", doing: "Finding journeys" },
  titles: { go: "Write the changes", doing: "Writing the changes" },
}
const took = (n: number) => (n < 60 ? `${n}s` : `${Math.floor(n / 60)}m ${n % 60}s`)

/** A control that starts a run, with Soba's own surfaces before and after it. */
export function RunControl({ task, lead, run, start, soba }: { task: Task; lead: string; run: Run | null; start: (t: Task) => void; soba: boolean }) {
  const t = TASKS[task]
  if (run?.running) {
    return (
      <div className="run" aria-live="polite">
        <div className="run-m">
          {run.task === task ? <span className="run-on"><i />{t.doing}, {took(run.secs)}</span> : <span>{lead}</span>}
          <small>{run.task === task ? `${run.step ? `${run.step}. ` : ""}You can leave this page. It keeps going, and the page fills in when it's done.` : `${TASKS[run.task].doing} first.`}</small>
        </div>
      </div>
    )
  }
  const error = run?.task === task ? asError(run) : null
  return (
    <>
      {soba && <ConnectNudge />}
      <div className="run" aria-live="polite">
        <div className="run-m">
          <span>{error ? "That stopped before it finished." : lead}</span>
          {!soba && error && <small className="bad-t">{error.error.message}</small>}
          {!error && <small>{soba ? "Runs on the computer you connected and can take a few minutes." : "Uses your Claude plan and can take a few minutes."}</small>}
        </div>
        <button className="bar-s" onClick={() => start(task)}>{error ? "Try again" : t.go}</button>
      </div>
      {soba && error && (
        <div style={{ padding: "0 16px 14px" }}>
          <RunLimit error={error} />
          <RunError error={error} />
        </div>
      )}
    </>
  )
}
