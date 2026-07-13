// True 5×7 dot-matrix display — each character is a grid of LED dots; unlit
// dots stay faintly visible like a real display. Color via `currentColor`
// (wrap in a styled span or pass `color`). Ported from the Hardware Bay mock.

const FONT: Record<string, number[]> = {
  "0": [14, 17, 19, 21, 25, 17, 14],
  "1": [4, 12, 4, 4, 4, 4, 14],
  "2": [14, 17, 1, 2, 4, 8, 31],
  "3": [31, 2, 4, 2, 1, 17, 14],
  "4": [2, 6, 10, 18, 31, 2, 2],
  "5": [31, 16, 30, 1, 1, 17, 14],
  "6": [6, 8, 16, 30, 17, 17, 14],
  "7": [31, 1, 2, 4, 8, 8, 8],
  "8": [14, 17, 17, 14, 17, 17, 14],
  "9": [14, 17, 17, 15, 1, 2, 12],
  A: [14, 17, 17, 31, 17, 17, 17],
  B: [30, 17, 17, 30, 17, 17, 30],
  C: [14, 17, 16, 16, 16, 17, 14],
  D: [30, 17, 17, 17, 17, 17, 30],
  E: [31, 16, 16, 30, 16, 16, 31],
  F: [31, 16, 16, 30, 16, 16, 16],
  G: [14, 17, 16, 23, 17, 17, 15],
  H: [17, 17, 17, 31, 17, 17, 17],
  I: [14, 4, 4, 4, 4, 4, 14],
  J: [7, 2, 2, 2, 2, 18, 12],
  K: [17, 18, 20, 24, 20, 18, 17],
  L: [16, 16, 16, 16, 16, 16, 31],
  M: [17, 27, 21, 21, 17, 17, 17],
  N: [17, 25, 21, 19, 17, 17, 17],
  O: [14, 17, 17, 17, 17, 17, 14],
  P: [30, 17, 17, 30, 16, 16, 16],
  Q: [14, 17, 17, 17, 21, 18, 13],
  R: [30, 17, 17, 30, 20, 18, 17],
  S: [15, 16, 16, 14, 1, 1, 30],
  T: [31, 4, 4, 4, 4, 4, 4],
  U: [17, 17, 17, 17, 17, 17, 14],
  V: [17, 17, 17, 17, 17, 10, 4],
  W: [17, 17, 17, 21, 21, 21, 10],
  X: [17, 17, 10, 4, 10, 17, 17],
  Y: [17, 17, 10, 4, 4, 4, 4],
  Z: [31, 1, 2, 4, 8, 16, 31],
  ":": [0, 6, 6, 0, 6, 6, 0],
  "-": [0, 0, 0, 14, 0, 0, 0],
  ".": [0, 0, 0, 0, 0, 6, 6],
  "+": [0, 4, 4, 31, 4, 4, 0],
  " ": [0, 0, 0, 0, 0, 0, 0],
};

const BLANK = [0, 0, 0, 0, 0, 0, 0];

interface Props {
  /** the text to render; uppercased, unknown glyphs render dark */
  text: string;
  /** LED diameter in px (dot pitch scales with it) */
  dot?: number;
  /** lit dots cast a soft glow */
  glow?: boolean;
  /** CSS color for lit dots; defaults to currentColor */
  color?: string;
  className?: string;
}

export function DotMatrix({ text, dot = 4, glow = true, color, className = "" }: Props) {
  return (
    <span
      className={`dm ${glow ? "dm-glow" : ""} ${className}`}
      style={{ "--dot": `${dot}px`, color } as React.CSSProperties}
      role="img"
      aria-label={text}
    >
      {[...text.toUpperCase()].map((ch, i) => {
        const bits = FONT[ch] ?? BLANK;
        return (
          <span key={i} className="dm-ch" aria-hidden>
            {bits.flatMap((row, r) =>
              [4, 3, 2, 1, 0].map((c) => (
                <i key={`${r}-${c}`} className={(row >> c) & 1 ? "on" : undefined} />
              )),
            )}
          </span>
        );
      })}
    </span>
  );
}
