// `mock.module` is global for the whole test run, so two test files each
// mocking "../api" clobber one another — the second registration wins and the
// first file's endpoints vanish. The API is therefore mocked exactly once,
// here, and tests steer it through these handles.
//
// The confirm dialog is NOT mocked: components render inside the real
// ConfirmProvider, which talks to this mocked API. That keeps one truth for
// "what does guard() do" and lets the provider's own tests live here too.
import { mock } from "bun:test";
import type {
  Catalog,
  MeHistory,
  MeToday,
  MyRules,
  Policy,
  WhereData,
  AuthConfig,
  CodeRequest,
  ConfirmGrant,
  ConfirmStatus,
  FamilyResponse,
  LockResponse,
  RecoveryCodes,
  RecoveryCodesStatus,
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
  confirmStatus: 0,
  verify: [] as string[],
  codeStart: [] as string[],
  codeVerify: [] as string[],
  register: [] as [string, string | undefined][],
  passkey: 0,
  unlockCode: [] as string[],
  recoveryCodes: [] as string[],
  generateRecovery: [] as string[],
  rotate: [] as string[],
  credit: [] as [string, number][],
  profileSaves: [] as { id: string; policy: Policy }[],
  myRules: [] as MyRules[],
  where: [] as (string | undefined)[],
};

export const MOCK_CODE = "123456";

function inMinutes(m: number): string {
  return new Date(Date.now() + m * 60_000).toISOString();
}

export const apiImpl = {
  getFamily: (() => Promise.reject(new Error("no getFamily impl set"))) as () => Promise<FamilyResponse>,
  getAuthConfig: (() =>
    Promise.resolve({
      oidc: false,
      oidc_name: "SSO",
      needs_setup: false,
      setup_code_required: false,
    })) as () => Promise<AuthConfig>,
  lockDevice: ((_: string) => Promise.resolve(ok)) as Lock,
  unlockDevice: ((_: string) => Promise.resolve(ok)) as Lock,
  getConfirmStatus: (() =>
    Promise.resolve({ armed_until: null, passkey: false, computer: true })) as () => Promise<ConfirmStatus>,
  startConfirmCode: (() =>
    Promise.resolve({ request_id: "r1", expires_in_secs: 300 })) as () => Promise<CodeRequest>,
  verifyConfirmCode: ((_: string, code: string) =>
    code === MOCK_CODE
      ? Promise.resolve({ armed_until: inMinutes(15) })
      : Promise.reject(new ApiError("wrong_code", "that code didn't match", 401))) as (
    id: string,
    c: string,
  ) => Promise<ConfirmGrant>,
  confirmWithPasskey: (() =>
    Promise.resolve({ armed_until: inMinutes(15) })) as () => Promise<ConfirmGrant>,
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
  // The person page and the person's own page.
  getMeToday: (() => Promise.reject(new Error("no getMeToday impl set"))) as () => Promise<MeToday>,
  getMeHistory: (() => Promise.resolve({ days: [], today_by_device: [] })) as () => Promise<MeHistory>,
  getMyRules: (() =>
    Promise.resolve({ daily_limit_minutes: 0, focus_hours: null, sites: [] })) as () => Promise<MyRules>,
  setMyRules: ((r: MyRules) => Promise.resolve(r)) as (r: MyRules) => Promise<MyRules>,
  getWhere: ((_?: string) =>
    Promise.resolve({ apps: [], sites: [], hours: [] })) as (accountId?: string) => Promise<WhereData>,
  getCatalog: (() => Promise.resolve({ categories: [], apps: [] })) as () => Promise<Catalog>,
};

const defaults = { ...apiImpl };

/** The confirm window open from the first render (the server says so on mount). */
export function armConfirm(minutes = 15) {
  apiImpl.getConfirmStatus = () =>
    Promise.resolve({ armed_until: inMinutes(minutes), passkey: false, computer: true });
}

export function resetApiMock() {
  apiCalls.family = 0;
  apiCalls.locked.length = 0;
  apiCalls.unlocked.length = 0;
  apiCalls.confirmStatus = 0;
  apiCalls.verify.length = 0;
  apiCalls.codeStart.length = 0;
  apiCalls.codeVerify.length = 0;
  apiCalls.register.length = 0;
  apiCalls.passkey = 0;
  apiCalls.unlockCode.length = 0;
  apiCalls.recoveryCodes.length = 0;
  apiCalls.generateRecovery.length = 0;
  apiCalls.rotate.length = 0;
  apiCalls.credit.length = 0;
  apiCalls.profileSaves.length = 0;
  apiCalls.myRules.length = 0;
  apiCalls.where.length = 0;
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
  getConfirmStatus: () => {
    apiCalls.confirmStatus += 1;
    return apiImpl.getConfirmStatus();
  },
  startConfirmCode: () => apiImpl.startConfirmCode(),
  verifyConfirmCode: (id: string, c: string) => {
    apiCalls.verify.push(c);
    return apiImpl.verifyConfirmCode(id, c);
  },
  confirmWithPasskey: () => apiImpl.confirmWithPasskey(),
  // The sign-in page and the session provider.
  getAuthConfig: () => apiImpl.getAuthConfig(),
  getMe: () => Promise.reject(new ApiError("unauthorized", "no session", 401)),
  pkcePair: () => Promise.resolve({ verifier: "v".repeat(43), challenge: "c".repeat(43) }),
  auth: {
    logout: () => Promise.resolve(),
    voucher: () => Promise.resolve(),
    link: () => Promise.resolve(),
    register: (name: string, setupToken?: string) => {
      apiCalls.register.push([name, setupToken]);
      return Promise.resolve();
    },
    passkey: () => {
      apiCalls.passkey += 1;
      return Promise.resolve();
    },
    codeStart: (name: string) => {
      apiCalls.codeStart.push(name);
      return Promise.resolve({ request_id: "r1", expires_in_secs: 300 });
    },
    codeVerify: (_id: string, _verifier: string, code: string) => {
      apiCalls.codeVerify.push(code);
      return code === MOCK_CODE
        ? Promise.resolve()
        : Promise.reject(new ApiError("wrong_code", "that code didn't match — check your computer and try again", 401));
    },
  },
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
  // The person page.
  listEvents: () => Promise.resolve([]),
  getCatalog: () => apiImpl.getCatalog(),
  getWhere: (accountId?: string) => {
    apiCalls.where.push(accountId);
    return apiImpl.getWhere(accountId);
  },
  creditTime: (du: string, minutes: number) => {
    apiCalls.credit.push([du, minutes]);
    return Promise.resolve();
  },
  updateProfile: (id: string, policy: Policy) => {
    apiCalls.profileSaves.push({ id, policy });
    return Promise.resolve({ id, policy });
  },
  updateMember: (id: string) => Promise.resolve({ id }),
  deleteMember: () => Promise.resolve(),
  unblockMember: () => Promise.resolve(),
  approveEarnRequest: (id: string) => Promise.resolve({ id }),
  denyEarnRequest: (id: string) => Promise.resolve({ id }),
  // The person's own page.
  getMeToday: () => apiImpl.getMeToday(),
  getMeHistory: () => apiImpl.getMeHistory(),
  askForTime: () => Promise.resolve(),
  getMyRules: () => apiImpl.getMyRules(),
  setMyRules: (r: MyRules) => {
    apiCalls.myRules.push(r);
    return apiImpl.setMyRules(r);
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
