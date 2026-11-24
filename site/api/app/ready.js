// GET /api/app/ready?provider=google|apple: whether the apps' sign-in with that provider is set up (APP-1128).
import { appDeps } from "../_lib/deps.js";
import { appReady } from "../_lib/app.js";

export async function GET(request) {
  return appReady(request, await appDeps());
}
