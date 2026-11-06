import Script from "next/script"
import { BODY } from "./body.generated"
import { SobaDock } from "./soba-dock"

export default function Page() {
  return (
    <>
      <div style={{ display: "contents" }} suppressHydrationWarning dangerouslySetInnerHTML={{ __html: BODY }} />
      <SobaDock />
      <Script src="/flow-page.js" strategy="afterInteractive" />
    </>
  )
}
