// GET /app/callback, rewritten here by vercel.json: the page Android's sign-in returns to when the app didn't open (REL-440).
import { readAppConfig } from "../_lib/deps.js";
import { appLanding } from "../_lib/app.js";

export function GET(request) {
  return appLanding(request, { config: readAppConfig(process.env) });
}
