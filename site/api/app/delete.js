// POST /api/app/delete: deletes the FERB account and everything kept for it (REL-440, REL-441).
import { appDeps } from "../_lib/deps.js";
import { appDelete } from "../_lib/app.js";

export async function POST(request) {
  return appDelete(request, await appDeps());
}
