// GET and POST /api/unsubscribe?t=TOKEN: the one-click unsubscribe in every update email (REL-433).
import { deps } from "./_lib/deps.js";
import { unsubscribe } from "./_lib/handlers.js";

export async function GET(request) {
  return unsubscribe(request, await deps());
}

export async function POST(request) {
  return unsubscribe(request, await deps());
}
