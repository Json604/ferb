// GET /download/FILE?t=TOKEN, rewritten here by vercel.json: the signed, short-lived download link (REL-433).
import { deps } from "./_lib/deps.js";
import { download } from "./_lib/handlers.js";

export async function GET(request) {
  return download(request, await deps());
}
