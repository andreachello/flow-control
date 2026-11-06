import Script from "next/script"
import { BODY } from "./body.generated"
import { FlowApp } from "./flow-app"

export default function Page() {
  return (
    <>
      <div style={{ display: "contents" }} suppressHydrationWarning dangerouslySetInnerHTML={{ __html: BODY }} />
      <FlowApp />
      <Script src="/flow-page.js" strategy="afterInteractive" />
    </>
  )
}
