// POST /api/app/signout: ends an app's sign-in (REL-440, REL-441).
import { appDeps } from "../_lib/deps.js";
import { appSignOut } from "../_lib/app.js";

export async function POST(request) {
  return appSignOut(request, await appDeps());
}
