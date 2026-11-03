import { S } from "./state";

// Feature flags live in state.
export function isEnabled(name: string) {
  return !!S.flags[name];
}
