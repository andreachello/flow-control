export type Stop = { say: string }
export type Journey = { title: string; icon: string; stops?: Stop[]; product?: string | null }
export type Commit = { id: string; title: string; author: string; date: string; quiet?: boolean }
export type Summary = { journeys: unknown[]; focus: number[]; quiet?: boolean; line?: string }
export type Page = { commits: Commit[]; size: number; stamp: number; journeys: Journey[]; project: { changes: number } }

/** Same answer as the screens not yet in React, so a sign-out clears both. */
const kept = <T,>(key: string): T | null => {
  try {
    return JSON.parse(sessionStorage.getItem(key) ?? "null")
  } catch {
    return null
  }
}
const keep = (key: string, value: unknown) => {
  try {
    sessionStorage.setItem(key, JSON.stringify(value))
  } catch {}
}

/** The product this tab is reading, picked in the sidebar. Empty is all of them. */
export const pickedProduct = (project: string, book: Journey[]) => {
  const p = kept<string>(`flow-product:${project}`)
  return p && book.some(j => j.product === p) ? p : ""
}

export class SignedOut extends Error {}

export async function get<T>(url: string): Promise<T> {
  const r = await fetch(url)
  if (r.status === 401) throw new SignedOut("Sign in first")
  const d = await r.json()
  if (!r.ok) throw new Error(d.error || "Something went wrong")
  return d
}

export const api = (project: string, what: string, q = "") => `/api/p/${encodeURIComponent(project)}/${what}${q}`
export const link = (project: string, ...parts: string[]) => "#/" + ["p", encodeURIComponent(project), ...parts].join("/")

/** The list comes straight from git. What each change reaches takes a lift, so it arrives after. */
export async function readChanges(project: string, have: Record<string, Summary>, stamp: number, list: Commit[], update: (s: Record<string, Summary>) => void, live: () => boolean) {
  const key = `flow-summ:${project}:${stamp}`
  const all = { ...(kept<Record<string, Summary>>(key) ?? {}), ...have }
  // What is already worked out arrives in one go, so the page is drawn once with its real numbers.
  const known = list.filter(c => !all[c.id]).map(c => c.id)
  if (known.length) {
    try {
      Object.assign(all, await get<Record<string, Summary>>(api(project, "summaries", `?saved=1&ids=${known.join(",")}`)))
    } catch {}
    if (!live()) return
  }
  update({ ...all })
  for (;;) {
    const ids = list.filter(c => !all[c.id]).slice(0, 4).map(c => c.id)
    if (!live()) return
    if (!ids.length) return keep(key, all)
    try {
      Object.assign(all, await get<Record<string, Summary>>(api(project, "summaries", `?ids=${ids.join(",")}`)))
    } catch {
      for (const id of ids) all[id] = { journeys: [], focus: [], line: "Could not read this change" }
    }
    if (live()) update({ ...all })
  }
}
