import { runPlugin } from "@/lib/plugins";

// Run a plugin chosen by the request.
export async function GET(req: Request) {
  const name = new URL(req.url).searchParams.get("name") ?? "summarize";
  return Response.json({ out: runPlugin(name) });
}
