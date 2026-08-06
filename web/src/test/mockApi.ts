// `mock.module` is global for the whole test run, so two test files each
// mocking "../api" clobber one another — the second registration wins and the
// first file's endpoints vanish. The API is therefore mocked exactly once,
// here, and tests steer it through these handles.
import { mock } from "bun:test";
import type { FamilyResponse, LockResponse } from "../types";

type Lock = (id: string) => Promise<LockResponse>;

const ok: LockResponse = { command_id: "c", queued: true, delivered: true };

export const apiCalls = {
  family: 0,
  locked: [] as string[],
  unlocked: [] as string[],
};

export const apiImpl = {
  getFamily: (() => Promise.reject(new Error("no getFamily impl set"))) as () => Promise<FamilyResponse>,
  lockDevice: ((_: string) => Promise.resolve(ok)) as Lock,
  unlockDevice: ((_: string) => Promise.resolve(ok)) as Lock,
};

export function resetApiMock() {
  apiCalls.family = 0;
  apiCalls.locked.length = 0;
  apiCalls.unlocked.length = 0;
  apiImpl.lockDevice = () => Promise.resolve(ok);
  apiImpl.unlockDevice = () => Promise.resolve(ok);
}

mock.module("../api", () => ({
  getFamily: () => {
    apiCalls.family += 1;
    return apiImpl.getFamily();
  },
  lockDevice: (id: string) => {
    apiCalls.locked.push(id);
    return apiImpl.lockDevice(id);
  },
  unlockDevice: (id: string) => {
    apiCalls.unlocked.push(id);
    return apiImpl.unlockDevice(id);
  },
}));

// Toast and step-up are mocked here for the same reason: one registration,
// shared handles, no ordering surprises between test files.
export const toasts: { msg: string; tone?: string }[] = [];

export class StepUpCancelled extends Error {
  constructor() {
    super("Step-up cancelled");
    this.name = "StepUpCancelled";
  }
}

export const stepUp = {
  guard: (<T,>(fn: () => Promise<T>) => fn()) as <T>(fn: () => Promise<T>) => Promise<T>,
};

export function resetUiMocks() {
  toasts.length = 0;
  stepUp.guard = <T,>(fn: () => Promise<T>) => fn();
}

mock.module("../lib/toast", () => ({
  useToast: () => ({
    toast: (msg: string, tone?: string) => toasts.push({ msg, tone }),
  }),
  errMsg: (e: unknown, fallback: string) =>
    e instanceof Error && e.message ? e.message : fallback,
}));

mock.module("../lib/stepup", () => ({
  useStepUp: () => ({ guard: <T,>(fn: () => Promise<T>) => stepUp.guard(fn) }),
  StepUpCancelled,
}));
