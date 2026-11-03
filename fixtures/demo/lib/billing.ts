// Pricing and plan changes.
import { S } from "./state";
import { isEnabled } from "./flags";
import { emit } from "./bus";

export function resolvePrice(plan: string) {
  const table: Record<string, number> = isEnabled("newPricing") ? { pro: 24 } : { pro: 20 };
  return table[plan];
}

export function chargeCard(amount: number) {
  S.charged = (S.charged ?? 0) + amount;
}

export function applyPlanChange(plan: "free" | "pro") {
  S.plans = { ...S.plans, [S.user]: { plan, interval: "month" } };
  emit("plan_changed");
}

export function planGuard(action: string) {
  if (S.plans[S.user].plan === "free") {
    throw new Error("upgrade required for " + action);
  }
}
