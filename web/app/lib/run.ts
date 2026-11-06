"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import { api, get } from "./project"

export type Task = "journeys" | "titles"
export type Run = { task: Task; running: boolean; failed: string | null; code: string | null; secs: number; step?: string | null }

/** A run goes on without the page, so this asks the server how it is going. */
export function useRun(project: string, onEnded: () => void) {
  const [run, setRun] = useState<Run | null>(null)
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined)
  const going = useRef(false)
  const ended = useRef(onEnded)
  ended.current = onEnded

  const follow = useCallback(async () => {
    clearTimeout(timer.current)
    let r: Run | null
    try {
      r = (await get<{ run: Run | null }>(api(project, "run"))).run
    } catch {
      return
    }
    setRun(r)
    if (going.current && r && !r.running) ended.current()
    going.current = !!r?.running
    if (r?.running) timer.current = setTimeout(follow, 2000)
  }, [project])

  useEffect(() => {
    follow()
    return () => clearTimeout(timer.current)
  }, [follow])

  const start = useCallback(async (task: Task) => {
    try {
      const r = await fetch(api(project, `run/${task}`), { method: "POST" })
      const d = await r.json()
      if (!r.ok) throw new Error(d.error || "Something went wrong")
      setRun(d.run)
      going.current = true
      follow()
    } catch (e) {
      setRun({ task, running: false, failed: e instanceof Error ? e.message : String(e), code: null, secs: 0 })
    }
  }, [project, follow])

  return { run, start }
}

/** Soba's components read a refusal by its code, in the shape its API sends. */
export const asError = (run: Run | null) =>
  run?.failed ? { error: { code: run.code ?? undefined, message: run.failed.replace(/^.*?soba:[a-z_]+: /, "") } } : null
