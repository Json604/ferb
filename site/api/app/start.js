// GET /api/app/start: the apps' sign-in goes to Google or Apple (REL-440, REL-441).
import { appDeps } from "../_lib/deps.js";
import { appStart } from "../_lib/app.js";

export async function GET(request) {
  return appStart(request, await appDeps());
}
