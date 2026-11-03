// All app state lives in one object, like a tiny in-memory database.
export type Plan = { plan: "free" | "pro"; interval: "month" | "year" };

export type State = {
  users: Record<string, { email: string }>;
  plans: Record<string, Plan>;
  sid: string;
  user: string;
  flags: Record<string, boolean>;
  interval?: "month" | "year";
  outbox?: string[];
  charged?: number;
  prompt?: string;
  tools?: string[];
  reply?: string;
  header?: string;
};

export const S: State = {
  users: { u1: { email: "a@x.com" } },
  plans: { u1: { plan: "free", interval: "month" } },
  sid: "sid-u1",
  user: "u1",
  flags: { newPricing: true, betaUi: true },
};
