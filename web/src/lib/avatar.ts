// The curated family-avatar palette (docs/DESIGN.md §2): eight warm pairs,
// picked deterministically by hash(seed) % 8 so a family of faces looks like
// one family — never the uncontrolled hues the old hsl(hueFor(seed)) produced.
const PAIRS: { bg: string; ink: string }[] = [
  { bg: "#fbe3dd", ink: "#9a3b28" }, // blush
  { bg: "#fce6cf", ink: "#8a5a12" }, // peach
  { bg: "#f7efc9", ink: "#6f5a10" }, // butter
  { bg: "#dfeecf", ink: "#3f6a2a" }, // sage
  { bg: "#d6efe0", ink: "#1f6b45" }, // mint
  { bg: "#d9e9f5", ink: "#2b5878" }, // sky
  { bg: "#e2e2f7", ink: "#3f3f86" }, // iris
  { bg: "#efe0f2", ink: "#6d3577" }, // lilac
];

/** The disc colours (background + monogram ink) for a person's avatar. */
export function avatarColors(seed: string): { bg: string; ink: string } {
  let h = 0;
  for (let i = 0; i < seed.length; i++) h = (h * 31 + seed.charCodeAt(i)) >>> 0;
  return PAIRS[h % PAIRS.length];
}
