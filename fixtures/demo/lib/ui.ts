import { S } from "./state";

// The header badge.
export function renderHeader() {
  S.header = S.plans[S.user].plan.toUpperCase();
}
