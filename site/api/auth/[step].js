// /api/auth/google and /api/auth/callback: the owner's sign-in to the user list, in one function (REL-450).
import { AUTH_ROUTES, dispatch } from "../_lib/routes.js";

export const GET = (request) => dispatch(AUTH_ROUTES, request);
