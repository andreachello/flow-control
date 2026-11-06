export const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`

export const when = (d: string) => {
  const x = new Date(d + "T12:00:00")
  const year = x.getFullYear() === new Date().getFullYear() ? {} : { year: "numeric" as const }
  return x.toLocaleDateString("en-US", { month: "short", day: "numeric", ...year })
}

export const ago = (d: string) => {
  const today = new Date(new Date().toDateString()).getTime()
  const n = Math.round((today - new Date(d + "T00:00:00").getTime()) / 864e5)
  return n === 0 ? "Today" : n === 1 ? "Yesterday" : n > 0 && n < 7 ? `${n}d ago` : when(d)
}

export const iso = (d: Date) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`

export const monday = (d: string) => {
  const x = new Date(d + "T12:00:00")
  x.setDate(x.getDate() - ((x.getDay() + 6) % 7))
  return x
}
