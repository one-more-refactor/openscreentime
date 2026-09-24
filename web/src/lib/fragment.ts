// One-time tokens arrive in the URL fragment, which the browser never sends to
// a server — so they can't land in an access log on the way in:
//
//   #v=…       a device voucher from `ost login`
//   #signin=…  a recovery link from `openscreentime-server recover`
//   #setup=…   the first-run link the installer printed
//
// Each is read once and removed from the address bar before anything else
// happens (replaceState, so there's no history entry to go Back to).

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
