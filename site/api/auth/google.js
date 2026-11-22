// GET /api/auth/google: send the person to Google's sign-in (REL-433).
import { deps } from "../_lib/deps.js";
import { startSignIn } from "../_lib/handlers.js";

export async function GET(request) {
  return startSignIn(request, await deps());
}
