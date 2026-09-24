// The editor must say exactly what the agent will do. Both sides are held to
// the same vectors: policy/tests/schedule-vectors.json (the Rust test
// `shared_schedule_vectors_agree` reads the same file).
import { describe, expect, test } from "bun:test";
import vectors from "../../../policy/tests/schedule-vectors.json";
import { bedtimeProblem, describeWindow, parseHm, windowProblem } from "./schedule";

describe("allowed hours", () => {
  for (const w of vectors.windows) {
    test(`${w.start}–${w.end} is ${w.valid ? "valid" : "refused"}`, () => {
      expect(windowProblem(w.start, w.end) === null).toBe(w.valid);
      if ("label" in w && w.label) expect(describeWindow(w.start, w.end)).toBe(w.label);
    });
  }
});

describe("bedtime", () => {
  for (const b of vectors.bedtimes) {
    test(`${b.start}–${b.end} is ${b.valid ? "valid" : "refused"}`, () => {
      expect(bedtimeProblem(b.start, b.end) === null).toBe(b.valid);
    });
  }
});

test("times parse like the agent parses them", () => {
  expect(parseHm("07:30")).toBe(450);
  expect(parseHm("7:05")).toBe(425);
  expect(parseHm("00:00")).toBe(0);
  expect(parseHm("24:00")).toBeNull();
  expect(parseHm("")).toBeNull();
});
