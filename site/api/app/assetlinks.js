// GET /.well-known/assetlinks.json, rewritten here by vercel.json: Android App Links for the sign-in's return (REL-440).
import { readAppConfig } from "../_lib/deps.js";
import { assetLinks } from "../_lib/app.js";

export function GET(request) {
  return assetLinks(request, { config: readAppConfig(process.env) });
}
