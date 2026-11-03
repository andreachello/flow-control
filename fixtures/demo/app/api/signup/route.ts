import { validateForm, createUser, issueSid, loadSession } from "@/lib/auth";
import { sendEmail } from "@/lib/mailer";
import { renderHeader } from "@/lib/ui";

// A new person signs up.
export async function POST(req: Request) {
  const f = validateForm(await req.json());
  const id = createUser(f);
  issueSid(id);
  loadSession();
  sendEmail("welcome");
  renderHeader();
  return Response.json({ ok: true });
}
