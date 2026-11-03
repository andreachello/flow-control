import { validateForm, issueSid, loadSession } from "@/lib/auth";
import { renderHeader } from "@/lib/ui";

// Someone logs back in.
export async function POST(req: Request) {
  validateForm(await req.json());
  issueSid("u1");
  loadSession();
  renderHeader();
  return Response.json({ ok: true });
}
