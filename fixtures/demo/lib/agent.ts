// The agent: prompt, tools and reply.
import { S } from "./state";
import { isEnabled } from "./flags";

export function buildSystemPrompt() {
  const base = "You help a " + S.plans[S.user].plan + " user.";
  S.prompt = isEnabled("betaUi") ? base + " Beta on." : base;
}

export function pickTools() {
  S.tools = S.plans[S.user].plan === "free" ? ["search"] : ["search", "code"];
}

export function streamReply() {
  S.reply = "ok:" + S.prompt!.length + ":" + S.tools!.length;
}
