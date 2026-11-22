// GET /api/admin: the owner's list of people who signed in, and its CSV (REL-433).
import { deps } from "./_lib/deps.js";
import { admin } from "./_lib/handlers.js";

export async function GET(request) {
  return admin(request, await deps());
}
