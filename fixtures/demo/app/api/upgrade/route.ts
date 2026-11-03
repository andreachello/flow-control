import { loadSession } from "@/lib/auth";
import { chargeCard, resolvePrice, applyPlanChange } from "@/lib/billing";
import { renderHeader } from "@/lib/ui";

// A free user upgrades to Pro.
export async function POST(req: Request) {
  const { plan } = await req.json();
  loadSession();
  chargeCard(resolvePrice(plan));
  applyPlanChange(plan);
  renderHeader();
  return Response.json({ ok: true });
}
