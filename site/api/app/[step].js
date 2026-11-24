// /api/app/<step>: the apps' sign-in endpoints in one function (api/_lib/routes.js has the table; REL-440, REL-441).
import { APP_ROUTES, dispatch } from "../_lib/routes.js";

export const GET = (request) => dispatch(APP_ROUTES, request);
export const POST = (request) => dispatch(APP_ROUTES, request);
