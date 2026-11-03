import { loadSession } from "@/lib/auth";
import { applyPlanChange } from "@/lib/billing";
import { renderHeader } from "@/lib/ui";

// A Pro user cancels.
export async function POST() {
  loadSession();
  applyPlanChange("free");
  renderHeader();
  return Response.json({ ok: true });
}
