// Email. The plan-changed handler is wired by name.
import { S } from "./state";
import { on } from "./bus";

export function sendEmail(kind: string) {
  const u = S.users[S.user];
  S.outbox = [...(S.outbox ?? []), `${kind} -> ${u.email}`];
}

function onPlanChanged() {
  sendEmail("plan-changed");
}

on("plan_changed", onPlanChanged);
