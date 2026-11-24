// GET (Google) and POST (Apple's form_post) /api/app/callback: the provider answers the apps' sign-in (REL-440, REL-441).
import { appDeps } from "../_lib/deps.js";
import { appCallback } from "../_lib/app.js";

export async function GET(request) {
  return appCallback(request, await appDeps());
}

export async function POST(request) {
  return appCallback(request, await appDeps());
}
