// ============================================================================
// The day, in sentences — what the rules verdict the server computed (the
// agent's own rules function, on the computer's clock) means for a person.
// Shared by the person page and the person's own page so both say the same
// thing about the same moment, only the pronoun differs.
// ============================================================================
import type { RulesVerdict } from "../types";

/** "20:00", or "tomorrow at 07:00", or "Sat at 09:00" — 24-hour, like every
 * time the console shows. */
export function whenLabel(iso: string, now = new Date()): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const hm = `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diff = Math.round((day(d) - day(now)) / 86_400_000);
  if (diff === 0) return hm;
  if (diff === 1) return `tomorrow at ${hm}`;
  return `${d.toLocaleDateString(undefined, { weekday: "short" })} at ${hm}`;
}

type Who = "they" | "you";

/** One sentence about what stops the screen next, or null when nothing does. */
export function stopSentence(r: RulesVerdict | null | undefined, who: Who, now = new Date()): string | null {
  if (!r) return null;
  const their = who === "they" ? "their" : "your";
  if (!r.allowed) {
    const until = r.resume_at ? whenLabel(r.resume_at, now) : null;
    switch (r.reason) {
      case "limit":
        return "Time's up for today. It starts again tomorrow.";
      case "bedtime":
        return until ? `Bedtime until ${until}.` : "It's bedtime.";
      case "outside_hours":
        return until ? `Outside ${their} allowed hours until ${until}.` : `Outside ${their} allowed hours.`;
      case "paused":
        return who === "they" ? "Paused." : "Your computer is paused.";
      default:
        return null;
    }
  }
  if (!r.stop_at) return null;
  const at = whenLabel(r.stop_at, now);
  switch (r.reason) {
    case "bedtime":
      return `Screens stop at ${at} for bedtime.`;
    case "outside_hours":
      return `Screens stop at ${at}, when ${their} allowed hours end.`;
    case "limit":
      return `If ${who} keep going, ${their} time runs out at ${at}.`;
    default:
      return null;
  }
}
