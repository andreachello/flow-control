"use client"

import { ConnectButton, SobaProvider, useSoba } from "@soba-so/react"
import { type ReactNode, useCallback, useEffect, useState } from "react"
import { createPortal } from "react-dom"
import { Overview } from "./screens/overview"

type Me = { soba: boolean; user: { login: string } | null }
type Screen = { name: "overview"; project: string; at: number } | null

declare global {
  interface Window {
    flowSoba?: { open: () => void }
    flowScreenNow?: Screen
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

/** The old router still picks the page. It names the ones React draws, and leaves them to this. */
export function FlowApp() {
  const [me, setMe] = useState<Me | null>(null)
  const [screen, setScreen] = useState<Screen>(null)
  const [slots, setSlots] = useState<{ button: HTMLElement; view: HTMLElement } | null>(null)
  const signedOut = useCallback(() => location.reload(), [])

  useEffect(() => {
    setSlots({ button: document.getElementById("soba-slot")!, view: document.getElementById("react-view")! })
    setScreen(window.flowScreenNow ?? null)
    const heard = (e: Event) => setScreen((e as CustomEvent<Screen>).detail)
    addEventListener("flow:screen", heard)
    fetch("/api/me").then(r => r.json()).then(setMe, () => setMe({ soba: false, user: null }))
    return () => removeEventListener("flow:screen", heard)
  }, [])

  if (!me || !slots) return null
  const soba = me.soba && !!me.user
  const page = screen && createPortal(<Overview key={`${screen.project}:${screen.at}`} project={screen.project} soba={soba} signedOut={signedOut} />, slots.view)
  const wrap = (children: ReactNode) =>
    soba ? (
      <SobaProvider sessionEndpoint="/api/soba/session" sessionCacheKey={me.user!.login}>
        <Bridge />
        {createPortal(<ConnectButton />, slots.button)}
        {children}
      </SobaProvider>
    ) : children
  return <>{wrap(page)}</>
}
