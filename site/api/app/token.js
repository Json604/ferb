// POST /api/app/token: an app trades its one-time code, or its refresh token, for tokens (REL-440, REL-441).
import { appDeps } from "../_lib/deps.js";
import { appToken } from "../_lib/app.js";

export async function POST(request) {
  return appToken(request, await appDeps());
}
