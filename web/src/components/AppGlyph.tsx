// ============================================================================
// AppGlyph — a stable icon for a catalog app (CONTRACT-0.6: icons anchor
// recognition; a grid of two-letter tiles anchors nothing).
//
// Brand glyphs come from the simple-icons package (bundled, nothing fetched);
// brands that project has purged for trademark reasons fall back to the
// deterministic monogram tile, which is at least *stable*: same app, same
// hue, same letters, every visit.
//
// Monochrome on purpose: glyphs render in currentColor so they sit inside the
// grayscale hierarchy instead of turning the rules page into a sticker sheet.
// ============================================================================
import {
  siDiscord,
  siEa,
  siEpicgames,
  siFacebook,
  siFortnite,
  siInstagram,
  siMessenger,
  siNetflix,
  siPinterest,
  siPlaystation,
  siReddit,
  siRiotgames,
  siRoblox,
  siSignal,
  siSnapchat,
  siSpotify,
  siSteam,
  siSupercell,
  siTelegram,
  siTiktok,
  siTinder,
  siTwitch,
  siWhatsapp,
  siX,
  siYoutube,
} from "simple-icons";

interface SimpleIcon {
  path: string;
  title: string;
}

/** catalog app id → bundled brand glyph. Absent = monogram fallback. */
const GLYPHS: Record<string, SimpleIcon> = {
  youtube: siYoutube,
  tiktok: siTiktok,
  instagram: siInstagram,
  snapchat: siSnapchat,
  facebook: siFacebook,
  x: siX,
  reddit: siReddit,
  pinterest: siPinterest,
  discord: siDiscord,
  whatsapp: siWhatsapp,
  telegram: siTelegram,
  signal: siSignal,
  messenger: siMessenger,
  twitch: siTwitch,
  netflix: siNetflix,
  spotify: siSpotify,
  roblox: siRoblox,
  fortnite: siFortnite,
  fortnite_epic: siEpicgames,
  steam: siSteam,
  riot: siRiotgames,
  ea: siEa,
  supercell: siSupercell,
  playstation: siPlaystation,
  tinder: siTinder,
};
// Purged brands (Minecraft, Xbox, Amazon, Disney+, ChatGPT, …) fall through
// to the monogram on purpose — a wrong mark is worse than a stable tile.

/** Deterministic warm hue per id — the same one the child avatars use. */
function hueFor(id: string): number {
  let h = 0;
  for (const ch of id) h = (h * 31 + ch.charCodeAt(0)) % 360;
  return h;
}

export function AppGlyph({ id, name, size = 34 }: { id: string; name: string; size?: number }) {
  const glyph = GLYPHS[id];
  if (glyph) {
    return (
      <span
        className="apps-mono apps-glyph"
        style={{ width: size, height: size }}
        aria-hidden="true"
      >
        <svg viewBox="0 0 24 24" width={size * 0.55} height={size * 0.55} fill="currentColor">
          <path d={glyph.path} />
        </svg>
      </span>
    );
  }
  const h = hueFor(id);
  const parts = name.replace(/[()]/g, "").split(/[\s/]+/).filter(Boolean);
  const mono = (parts.length >= 2 ? parts[0][0] + parts[1][0] : name.slice(0, 2)).toUpperCase();
  return (
    <span
      className="apps-mono"
      style={{
        width: size,
        height: size,
        background: `hsl(${h} 45% 88%)`,
        color: `hsl(${h} 55% 26%)`,
      }}
      aria-hidden="true"
    >
      {mono}
    </span>
  );
}
