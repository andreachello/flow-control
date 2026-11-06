import type { Metadata } from "next"
import type { ReactNode } from "react"
import "@soba-so/react/styles.css"
import "./flow.generated.css"

export const metadata: Metadata = { title: "Flow control" }

export default function Layout({ children }: { children: ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <link rel="preconnect" href="https://fonts.googleapis.com" />
        <link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Geist:wght@400;500;600&family=Geist+Mono:wght@400;500;600&display=swap" />
      </head>
      <body suppressHydrationWarning>{children}</body>
    </html>
  )
}
