// `mock.module` is global for the whole test run, so two test files each
// mocking "../api" clobber one another — the second registration wins and the
// first file's endpoints vanish. The API is therefore mocked exactly once,
// here, and tests steer it through these handles.
//
// Change mode is NOT mocked: components render inside the real
// ConfirmProvider, which talks to this mocked API. That keeps one truth for
// "what does guard() do" and lets the provider's own tests live here too.
import { mock } from "bun:test";
import type {
  ChangeModeStatus,
  FamilyResponse,
  LockResponse,
  RecoveryCodes,
  RecoveryCodesStatus,
  SecondFactorMethod,
  StepUpGrant,
  TwoFactorStatus,
  UnlockCode,
  UnlockCodeRotated,
} from "../types";

type Lock = (id: string) => Promise<LockResponse>;

const ok: LockResponse = { command_id: "c", queued: true, delivered: true };

/** The same shape api.ts's ApiError has; `instanceof` checks use THIS class. */
export class ApiError extends Error {
  code: string;
  status: number;
  constructor(code: string, message: string, status: number) {
    super(message);
    this.code = code;
    this.status = status;
    this.name = "ApiError";
  }
}

export const apiCalls = {
  family: 0,
  locked: [] as string[],
  unlocked: [] as string[],
  changeMode: 0,
  lockChangeMode: 0,
  extendChangeMode: 0,
  verify: [] as string[],
  unlockCode: [] as string[],
  recoveryCodes: [] as string[],
  generateRecovery: [] as string[],
  rotate: [] as string[],
};

export const MOCK_CODE = "123456";

function inMinutes(m: number): string {
  return new Date(Date.now() + m * 60_000).toISOString();
}

export const apiImpl = {
  getFamily: (() => Promise.reject(new Error("no getFamily impl set"))) as () => Promise<FamilyResponse>,
  lockDevice: ((_: string) => Promise.resolve(ok)) as Lock,
  unlockDevice: ((_: string) => Promise.resolve(ok)) as Lock,
  getChangeMode: (() => Promise.resolve({ armed_until: null, extended: false })) as () => Promise<ChangeModeStatus>,
  lockChangeMode: (() => Promise.resolve({ armed_until: null, extended: false })) as () => Promise<ChangeModeStatus>,
  extendChangeMode: (() =>
    Promise.resolve({ armed_until: inMinutes(15), extended: true })) as () => Promise<ChangeModeStatus>,
  verifyStepUp: ((method: SecondFactorMethod, code: string) =>
    code === MOCK_CODE
      ? Promise.resolve({ method, expires_at: inMinutes(15), extended: false })
      : Promise.reject(new ApiError("invalid_code", "That code didn't match.", 400))) as (
    m: SecondFactorMethod,
    c: string,
  ) => Promise<StepUpGrant>,
  getTwoFactorStatus: (() =>
    Promise.resolve({ totp_enrolled: true, email_available: true })) as () => Promise<TwoFactorStatus>,
  getUnlockCode: ((id: string) =>
    Promise.resolve({ code: "123456", seconds_left: 20, period: 30, device_name: id })) as (
    id: string,
  ) => Promise<UnlockCode>,
  rotateUnlockCode: ((id: string) =>
    Promise.resolve({
      code: "654321",
      seconds_left: 30,
      period: 30,
      device_name: id,
      recovery_codes_cleared: true,
    })) as (id: string) => Promise<UnlockCodeRotated>,
  generateRecoveryCodes: ((_: string) =>
    Promise.resolve({
      codes: Array.from({ length: 8 }, (_, i) => `1234 567${i}`),
      generated_at: new Date().toISOString(),
    })) as (id: string) => Promise<RecoveryCodes>,
  getRecoveryCodes: ((_: string) =>
    Promise.resolve({ unused: 0, total: 8, generated_at: null })) as (id: string) => Promise<RecoveryCodesStatus>,
};

const defaults = { ...apiImpl };

/** Change mode on from the first render (the server says so on mount). */
export function armChangeMode(minutes = 15) {
  apiImpl.getChangeMode = () => Promise.resolve({ armed_until: inMinutes(minutes), extended: false });
}

export function resetApiMock() {
  apiCalls.family = 0;
  apiCalls.locked.length = 0;
  apiCalls.unlocked.length = 0;
  apiCalls.changeMode = 0;
  apiCalls.lockChangeMode = 0;
  apiCalls.extendChangeMode = 0;
  apiCalls.verify.length = 0;
  apiCalls.unlockCode.length = 0;
  apiCalls.recoveryCodes.length = 0;
  apiCalls.generateRecovery.length = 0;
  apiCalls.rotate.length = 0;
  Object.assign(apiImpl, defaults);
}

mock.module("../api", () => ({
  ApiError,
  usingMock: false,
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
  getChangeMode: () => {
    apiCalls.changeMode += 1;
    return apiImpl.getChangeMode();
  },
  lockChangeMode: () => {
    apiCalls.lockChangeMode += 1;
    return apiImpl.lockChangeMode();
  },
  extendChangeMode: () => {
    apiCalls.extendChangeMode += 1;
    return apiImpl.extendChangeMode();
  },
  verifyStepUp: (m: SecondFactorMethod, c: string) => {
    apiCalls.verify.push(c);
    return apiImpl.verifyStepUp(m, c);
  },
  getTwoFactorStatus: () => apiImpl.getTwoFactorStatus(),
  startEmailStepUp: () => Promise.resolve(),
  startTelegramStepUp: () => Promise.resolve(),
  getUnlockCode: (id: string) => {
    apiCalls.unlockCode.push(id);
    return apiImpl.getUnlockCode(id);
  },
  rotateUnlockCode: (id: string) => {
    apiCalls.rotate.push(id);
    return apiImpl.rotateUnlockCode(id);
  },
  generateRecoveryCodes: (id: string) => {
    apiCalls.generateRecovery.push(id);
    return apiImpl.generateRecoveryCodes(id);
  },
  getRecoveryCodes: (id: string) => {
    apiCalls.recoveryCodes.push(id);
    return apiImpl.getRecoveryCodes(id);
  },
}));

// Toast is mocked here for the same reason: one registration, shared
// handles, no ordering surprises between test files.
export const toasts: { msg: string; tone?: string }[] = [];

export function resetUiMocks() {
  toasts.length = 0;
}

mock.module("../lib/toast", () => ({
  useToast: () => ({
    toast: (msg: string, tone?: string) => toasts.push({ msg, tone }),
  }),
  errMsg: (e: unknown, fallback: string) =>
    e instanceof Error && e.message ? e.message : fallback,
}));
