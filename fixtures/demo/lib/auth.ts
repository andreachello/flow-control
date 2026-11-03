// Sign-up and sessions.
import { S } from "./state";

export function validateForm(f: { email: string }) {
  if (f.email.indexOf("@") < 0) {
    throw new Error("bad email");
  }
  return f;
}

export function createUser(f: { email: string }) {
  const id = "u" + (Object.keys(S.users).length + 1);
  S.users = { ...S.users, [id]: { email: f.email } };
  S.plans = { ...S.plans, [id]: { plan: "free", interval: "month" } };
  return id;
}

export function issueSid(id: string) {
  S.sid = "sid-" + id;
}

export function loadSession() {
  S.user = S.sid.replace("sid-", "");
}
