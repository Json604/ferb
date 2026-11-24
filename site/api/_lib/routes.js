// The site's functions, by path: Vercel's Hobby plan deploys at most 12 functions, so each folder of endpoints is one
// function (api/app/[step].js, api/auth/[step].js) that picks its handler here. The addresses are unchanged (REL-452).
import { appDeps, deps, readAppConfig } from "./deps.js";
import {
  appAppleNative,
  appAssertion,
  appCallback,
  appDelete,
  appLanding,
  appMe,
  appReady,
  appSignOut,
  appStart,
  appToken,
  assetLinks,
} from "./app.js";
import { finishSignIn, startSignIn } from "./handlers.js";

const configOnly = () => ({ config: readAppConfig(process.env) });

// step -> method -> [handler, the deps it needs]
export const APP_ROUTES = {
  apple: { POST: [appAppleNative, appDeps] },
  assertion: { POST: [appAssertion, appDeps] },
  assetlinks: { GET: [assetLinks, configOnly] },
  callback: { GET: [appCallback, appDeps], POST: [appCallback, appDeps] },
  delete: { POST: [appDelete, appDeps] },
  landing: { GET: [appLanding, configOnly] },
  me: { GET: [appMe, appDeps] },
  ready: { GET: [appReady, appDeps] },
  signout: { POST: [appSignOut, appDeps] },
  start: { GET: [appStart, appDeps] },
  token: { POST: [appToken, appDeps] },
};

export const AUTH_ROUTES = {
  google: { GET: [startSignIn, deps] },
  callback: { GET: [finishSignIn, deps] },
};

// The paths vercel.json rewrites into api/app/[step] keep their own name in request.url on some routes; they map here.
const REWRITTEN = { "/.well-known/assetlinks.json": "assetlinks", "/app/callback": "landing" };

// Which step a request is for: Vercel passes the [step] segment as the `step` query parameter; otherwise the path's
// last segment. The parameter is removed before the handler sees the request, so the providers' answers are untouched.
export function stepOf(request) {
  const url = new URL(request.url);
  const step = url.searchParams.get("step") || REWRITTEN[url.pathname] || url.pathname.split("/").filter(Boolean).pop();
  url.searchParams.delete("step");
  return { step, url: url.toString() };
}

export async function dispatch(routes, request) {
  const { step, url } = stepOf(request);
  const byMethod = Object.hasOwn(routes, step || "") ? routes[step] : null;
  if (!byMethod) return new Response("Not found", { status: 404 });
  const route = byMethod[request.method];
  if (!route) return new Response("Method not allowed", { status: 405, headers: { allow: Object.keys(byMethod).join(", ") } });
  const [handler, needs] = route;
  const forwarded = url === request.url ? request : new Request(url, request);
  return handler(forwarded, await needs());
}
