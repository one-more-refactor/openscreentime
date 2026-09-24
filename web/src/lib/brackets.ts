import type { AgeBracket } from "../types";

/** What an age bracket means for the person, in a parent's words — said the
 * same when adding someone and when changing their age later. */
export const BRACKET_BLURB: Record<AgeBracket, string> = {
  little: "You decide everything. A firm daily limit and the simplest stop.",
  kid: "A firm limit and stop, but they can ask you for more time.",
  younger_teen: "Limits, two minutes' warning before the stop, and they can ask for more.",
  older_teen: "Mostly up to them. You see their apps and can still set a limit.",
  adult:
    "Keeps their own time: they set their own limit, focus hours and blocked sites on their own page. You see only their minutes.",
};
