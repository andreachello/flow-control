import { loadSession } from "@/lib/auth";
import { planGuard } from "@/lib/billing";
import * as agent from "@/lib/agent";

// A Pro user asks the agent.
export async function POST() {
  loadSession();
  planGuard("agent");
  agent.buildSystemPrompt();
  agent.pickTools();
  agent.streamReply();
  return Response.json({ ok: true });
}
