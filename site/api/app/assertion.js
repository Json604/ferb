// POST /api/app/assertion: the signed account statement a phone carries to its Mac when pairing (PHN-828).
import { appDeps } from "../_lib/deps.js";
import { appAssertion } from "../_lib/app.js";

export async function POST(request) {
  return appAssertion(request, await appDeps());
}
