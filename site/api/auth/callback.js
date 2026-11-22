// GET /api/auth/callback: Google sends the person back here (REL-433).
import { deps } from "../_lib/deps.js";
import { finishSignIn } from "../_lib/handlers.js";

export async function GET(request) {
  return finishSignIn(request, await deps());
}
