// GET /api/app/me: the signed-in account and its devices (REL-440, REL-441).
import { appDeps } from "../_lib/deps.js";
import { appMe } from "../_lib/app.js";

export async function GET(request) {
  return appMe(request, await appDeps());
}
