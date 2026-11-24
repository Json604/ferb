// POST /api/app/send-link: emails the download page's address to the account's own email (REL-440, REL-441).
import { appDeps } from "../_lib/deps.js";
import { appSendLink } from "../_lib/app.js";

export async function POST(request) {
  return appSendLink(request, await appDeps());
}
