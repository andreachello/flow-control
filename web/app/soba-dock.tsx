"use client"

import { ConnectButton, SobaProvider, useSoba } from "@soba-so/react"
import { useEffect, useState } from "react"
import { createPortal } from "react-dom"

type Me = { soba: boolean; user: { login: string } | null }

declare global {
  interface Window {
    flowSoba?: { open: () => void }
  }
}

/** Lets the screens not yet in React raise Soba's own connect flow. */
function Bridge() {
  const { connect } = useSoba()
  useEffect(() => {
    window.flowSoba = { open: () => connect.open() }
    return () => {
      delete window.flowSoba
    }
  }, [connect])
  return null
}

/** Only for someone signed in: a session is minted for a person, and the server decides who. */
export function SobaDock() {
  const [me, setMe] = useState<Me | null>(null)
  const [slot, setSlot] = useState<HTMLElement | null>(null)

  useEffect(() => {
    setSlot(document.getElementById("soba-slot"))
    fetch("/api/me").then(r => r.json()).then(setMe, () => setMe(null))
  }, [])

  if (!me?.soba || !me.user || !slot) return null
  return (
    <SobaProvider sessionEndpoint="/api/soba/session" sessionCacheKey={me.user.login}>
      <Bridge />
      {createPortal(<ConnectButton />, slot)}
    </SobaProvider>
  )
}
