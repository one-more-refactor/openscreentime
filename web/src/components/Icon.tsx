// ============================================================================
// Icon — the brand's one monoline set (brand/icons/*.svg: 24 grid, stroke 2,
// round caps and joins, currentColor).
//
// The SVG files are the single source: they are imported as text and put on
// the page as they are, never redrawn here. A new icon is drawn in
// brand/gen.py, written to brand/icons, and added to the map below (the test
// fails until it is). Emoji are never icons — they are people's faces.
// ============================================================================
import add from "../../../brand/icons/add.svg?raw";
import allowedHours from "../../../brand/icons/allowed-hours.svg?raw";
import apps from "../../../brand/icons/apps.svg?raw";
import arrowLeft from "../../../brand/icons/arrow-left.svg?raw";
import arrowRight from "../../../brand/icons/arrow-right.svg?raw";
import ask from "../../../brand/icons/ask.svg?raw";
import bell from "../../../brand/icons/bell.svg?raw";
import block from "../../../brand/icons/block.svg?raw";
import check from "../../../brand/icons/check.svg?raw";
import chevronDown from "../../../brand/icons/chevron-down.svg?raw";
import chevronRight from "../../../brand/icons/chevron-right.svg?raw";
import clock from "../../../brand/icons/clock.svg?raw";
import close from "../../../brand/icons/close.svg?raw";
import copy from "../../../brand/icons/copy.svg?raw";
import edit from "../../../brand/icons/edit.svg?raw";
import eyeOff from "../../../brand/icons/eye-off.svg?raw";
import family from "../../../brand/icons/family.svg?raw";
import giveTime from "../../../brand/icons/give-time.svg?raw";
import globe from "../../../brand/icons/globe.svg?raw";
import home from "../../../brand/icons/home.svg?raw";
import info from "../../../brand/icons/info.svg?raw";
import key from "../../../brand/icons/key.svg?raw";
import laptop from "../../../brand/icons/laptop.svg?raw";
import lock from "../../../brand/icons/lock.svg?raw";
import menu from "../../../brand/icons/menu.svg?raw";
import moon from "../../../brand/icons/moon.svg?raw";
import more from "../../../brand/icons/more.svg?raw";
import offline from "../../../brand/icons/offline.svg?raw";
import passkey from "../../../brand/icons/passkey.svg?raw";
import pause from "../../../brand/icons/pause.svg?raw";
import person from "../../../brand/icons/person.svg?raw";
import play from "../../../brand/icons/play.svg?raw";
import refresh from "../../../brand/icons/refresh.svg?raw";
import remove from "../../../brand/icons/remove.svg?raw";
import settings from "../../../brand/icons/settings.svg?raw";
import signOut from "../../../brand/icons/sign-out.svg?raw";
import stop from "../../../brand/icons/stop.svg?raw";
import sun from "../../../brand/icons/sun.svg?raw";
import unlock from "../../../brand/icons/unlock.svg?raw";
import warning from "../../../brand/icons/warning.svg?raw";
import week from "../../../brand/icons/week.svg?raw";

/** File name (without .svg) → the file's markup. */
export const ICONS = {
  add,
  "allowed-hours": allowedHours,
  apps,
  "arrow-left": arrowLeft,
  "arrow-right": arrowRight,
  ask,
  bell,
  block,
  check,
  "chevron-down": chevronDown,
  "chevron-right": chevronRight,
  clock,
  close,
  copy,
  edit,
  "eye-off": eyeOff,
  family,
  "give-time": giveTime,
  globe,
  home,
  info,
  key,
  laptop,
  lock,
  menu,
  moon,
  more,
  offline,
  passkey,
  pause,
  person,
  play,
  refresh,
  remove,
  settings,
  "sign-out": signOut,
  stop,
  sun,
  unlock,
  warning,
  week,
} as const;

export type IconName = keyof typeof ICONS;

interface Props {
  name: IconName;
  /** Edge length in px. 24 is the grid; 20 in nav rows, 18 in buttons, 16 inline. */
  size?: number;
  /** When the icon carries meaning on its own (no text beside it). */
  label?: string;
  className?: string;
}

/** One icon from the brand set, drawn in the current text colour. */
export function Icon({ name, size = 20, label, className = "" }: Props) {
  return (
    <span
      className={`ic ${className}`}
      style={{ width: size, height: size }}
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
      data-icon={name}
      // The file itself, as drawn — see the header.
      dangerouslySetInnerHTML={{ __html: ICONS[name] }}
    />
  );
}
