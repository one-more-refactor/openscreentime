// Input validation helpers shared by Login, Devices and the PolicyEditor.

const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

export function isEmail(v: string): boolean {
  return EMAIL_RE.test(v.trim());
}

// Domain: dot-separated labels of [a-z0-9-], no leading/trailing hyphen,
// optionally prefixed `*.`. Bare "*" (allow everything) is accepted too.
const LABEL = "[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?";
const DOMAIN_RE = new RegExp(`^(\\*\\.)?(${LABEL}\\.)*${LABEL}$`, "i");

export function isDomain(v: string): boolean {
  const s = v.trim();
  if (s === "*") return true;
  return s.length <= 253 && DOMAIN_RE.test(s) && s.includes(".");
}

export function isIpv4(v: string): boolean {
  const parts = v.trim().split(".");
  if (parts.length !== 4) return false;
  return parts.every((p) => /^\d{1,3}$/.test(p) && Number(p) <= 255);
}

export function isIpv6(v: string): boolean {
  const s = v.trim();
  if (!/^[0-9a-f:]+$/i.test(s) || !s.includes(":")) return false;
  const groups = s.split("::");
  if (groups.length > 2) return false;
  const valid = (part: string) =>
    part === "" ||
    part.split(":").every((g) => /^[0-9a-f]{1,4}$/i.test(g));
  return valid(groups[0]) && (groups.length === 1 || valid(groups[1]));
}

export function isIp(v: string): boolean {
  return isIpv4(v) || isIpv6(v);
}
