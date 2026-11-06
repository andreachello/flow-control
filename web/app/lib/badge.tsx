import type { Journey } from "./project"

const SHAPE: Record<string, string> = { enter:"M7 3h6v10H7M3 8h7M8 5l3 3-3 3", plus:"M8 3v10M3 8h10", cross:"M4 4l8 8M12 4l-8 8", up:"M8 13V3M4 7l4-4 4 4", spark:"M8 2l1.6 4.4L14 8l-4.4 1.6L8 14l-1.6-4.4L2 8l4.4-1.6z", plug:"M6 2v4M10 2v4M4 6h8v3a4 4 0 0 1-8 0zM8 13v2",
  home:"M3 7.500L8 3l5 4.500M4.5 6.500V13h7V6.5", calendar:"M3 4.500h10V13H3zM3 7.500h10M5.5 2.500v3M10.5 2.500v3", chat:"M3 3.500h10v7H7l-3 2.500v-2.500H3z", card:"M2.5 4.500h11v7h-11zM2.5 7.500h11", search:"M7 3a4 4 0 1 0 0 8 4 4 0 0 0 0-8zM10 10l3.5 3.5", key:"M6 7a3 3 0 1 0-.1.6M8.7 7.500H14M12 7.500v2.5", star:"M8 2.500l1.7 3.6 3.9.5-2.9 2.7.8 3.900L8 11.300l-3.5 1.9.8-3.9-2.9-2.7 3.9-.5z", photo:"M2.5 3.500h11v9h-11zM2.5 10.500l3-3 2.5 2.5 2-2 3.5 3.5", mail:"M2.5 4h11v8h-11zM2.5 4.500L8 9l5.5-4.5", chart:"M3 13V8M8 13V3M13 13V6", globe:"M8 2.500a5.5 5.5 0 1 0 0 11 5.5 5.5 0 0 0 0-11zM2.5 8h11M8 2.500c3 3 3 8 0 11M8 2.500c-3 3-3 8 0 11", people:"M6 7.500a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5zM1.5 13.500c.5-3 2-4 4.5-4s4 1 4.5 4M10.5 2.800a2.5 2.5 0 0 1 0 4.500M12 9.800c1.3.5 2 1.7 2.5 3.7", doc:"M4 2.500h5.500L12 5v8.500H4zM6 8h4M6 10.500h4", pin:"M8 14s4.5-4.3 4.5-7.500a4.5 4.5 0 0 0-9 0C3.5 9.7 8 14 8 14zM8 8a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3z", gear:"M8 5.500a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5zM8 1.800v2M8 12.200v2M1.8 8h2M12.2 8h2M3.6 3.600L5 5M11 11l1.4 1.400M3.6 12.400L5 11M11 5l1.4-1.4" }
const TINT = ["li", "su", "ca", "up", "ag", "te", "or", "in", "ol"]

export const tint = (g: number, book: Journey[]) => (book[g].icon === "gear" ? "var(--l-pl)" : `var(--l-${TINT[g % TINT.length]})`)

export function Badge({ g, book }: { g: number; book: Journey[] }) {
  return (
    <span className="bdg" style={{ background: tint(g, book) }} title={book[g].title}>
      <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        <path d={SHAPE[book[g].icon] || SHAPE.star} />
      </svg>
    </span>
  )
}
