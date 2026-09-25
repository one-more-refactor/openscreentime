// One-time tokens arrive in the URL fragment, which the browser never sends to
// a server — so they can't land in an access log on the way in:
//
//   #v=…       a device voucher from `ost login`
//   #signin=…  a recovery link from `openscreentime-server recover`
//   #setup=…   the first-run link the installer printed
//
// Each is read once and removed from the address bar before anything else
// happens (replaceState, so there's no history entry to go Back to).
//
// "Before anything else" means before the first redirect, too: a visit with no
// session to `/#setup=…` is sent on to `/login`, and that navigation replaces
// the whole URL — fragment and all. So the app takes every one-time token at
// its very first render (`captureFragmentTokens`) and hands each out once
// from memory (`takeToken`), wherever the router ends up.

const TOKEN = "[A-Za-z0-9_-]+";

export function readFragmentParam(hash: string, name: string): string | null {
  const m = new RegExp(`[#&]${name}=(${TOKEN})`).exec(hash);
  return m ? m[1] : null;
}

export function stripFragmentParam(hash: string, name: string): string {
  const rest = hash.replace(new RegExp(`[#&]${name}=${TOKEN}`), "");
  if (rest === "" || rest === "#") return "";
  return rest.startsWith("&") ? `#${rest.slice(1)}` : rest;
}

/** Read a token from the address bar and remove it there. */
export function takeFromFragment(name: string): string | null {
  const hash = window.location.hash;
  const token = readFragmentParam(hash, name);
  if (token === null) return null;
  window.history.replaceState(
    window.history.state,
    "",
    window.location.pathname + window.location.search + stripFragmentParam(hash, name),
  );
  return token;
}

/** The one-time tokens a link can carry. */
const ONE_TIME = ["v", "signin", "setup"] as const;

/** Taken from the address bar at startup, waiting for whoever redeems them. */
const captured = new Map<string, string>();

/** Take every one-time token out of the address bar now, before any redirect
 * can throw it away. Safe to call more than once. */
export function captureFragmentTokens(): void {
  for (const name of ONE_TIME) {
    const token = takeFromFragment(name);
    if (token !== null) captured.set(name, token);
  }
}

/** A one-time token — still in the address bar, or captured at startup —
 * handed out once. */
export function takeToken(name: string): string | null {
  const live = takeFromFragment(name);
  const kept = captured.get(name) ?? null;
  captured.delete(name);
  return live ?? kept;
}
