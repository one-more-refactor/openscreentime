import type { ParentSees } from "../types";

/** "What can a parent see?", said from what the server actually shows a
 * parent (`/api/me/today` → `parent_sees`, the same rule `/api/usage/where`
 * enforces). Nothing here decides by age: the server does, once.
 *
 * Without an answer (an older server), say the most a parent could see —
 * over-telling is honest, promising privacy the server doesn't keep is not. */
export function parentSeesSentence(p: ParentSees | undefined): string {
  const s = p ?? { apps: true, sites: true };
  if (!s.apps && !s.sites) {
    return "A parent sees how many minutes you used — not your apps, your sites or your own rules.";
  }
  const seen = ["how long your computer was used"];
  if (s.apps) seen.push("which apps were open");
  if (s.sites) seen.push("which sites it looked up");
  seen.push("when the rules kicked in");
  const not = [!s.apps && "your apps", !s.sites && "the sites you looked up"].filter(Boolean);
  const list = `${seen.slice(0, -1).join(", ")}, and ${seen[seen.length - 1]}`;
  return `Your parents can see ${list}${not.length ? ` — not ${not.join(" or ")}` : ""}.`;
}
