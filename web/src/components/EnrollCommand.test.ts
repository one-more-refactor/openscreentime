// The one line a parent pastes on a computer. Acceptance round 4: stock
// Debian has no curl, the line was curl only, and it "finished" with exit 0
// and nothing installed. The line is run here for real, by sh and by bash, on
// a PATH that holds only what that computer would have: curl, wget, both or
// neither (each a stand-in that serves the script from a file), `sudo` (runs
// the command as given), `sh` and a few coreutils.
import { afterAll, describe, expect, test } from "bun:test";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { installCommand } from "./EnrollCommand";

const HOST = "https://ost.example.org";
const STUB = 'echo "installer ran: OST_TOKEN=$OST_TOKEN args=$*"\n';
const REAL = readFileSync(new URL("../../../server/install.sh", import.meta.url), "utf8");
const WGET = `wget -qO- ${HOST}/install.sh`;
const CURL = `curl -fsSL ${HOST}/install.sh`;
const RAN = `installer ran: OST_TOKEN=T args=--server ${HOST}\n`;
const FAILED = `Couldn't download the installer from ${HOST} — is the address right and the server up?\n`;
const dirs: string[] = [];
afterAll(() => dirs.forEach((d) => rmSync(d, { recursive: true, force: true })));

function exe(path: string, body: string) {
  writeFileSync(path, `#!/bin/sh\n${body}`);
  chmodSync(path, 0o755);
}

/** A computer with these downloaders; the server answers with `script`, or
 * fails (curl's and wget's own exit codes) when it's null. */
function computer(tools: ("curl" | "wget")[], script: string | null = STUB) {
  const dir = mkdtempSync(join(tmpdir(), "ost-install-line-"));
  dirs.push(dir);
  const bin = join(dir, "bin");
  mkdirSync(bin);
  writeFileSync(join(dir, "install.sh"), script ?? "");
  for (const t of tools) {
    const fail = t === "curl" ? 22 : 8;
    exe(
      join(bin, t),
      `echo "${t} $*" >> "${dir}/calls"\n` +
        (script === null ? `echo "${t}: the server said no" >&2; exit ${fail}\n` : `cat "${dir}/install.sh"\n`),
    );
  }
  exe(join(bin, "sudo"), 'exec env "$@"\n');
  symlinkSync("/bin/sh", join(bin, "sh"));
  // What every Linux has (and the installer's first checks use) — but never
  // a curl or wget this computer isn't meant to have.
  for (const t of ["cat", "env", "id", "uname"]) symlinkSync(Bun.which(t)!, join(bin, t));
  const calls = () => {
    try {
      return readFileSync(join(dir, "calls"), "utf8").trim().split("\n");
    } catch {
      return [];
    }
  };
  return { bin, calls };
}

// sh (dash on Debian and Ubuntu, where it's installed), and bash — what a
// parent's terminal runs the pasted line in.
const shells = [...new Set(["/bin/sh", Bun.which("dash"), Bun.which("bash")])].filter(
  (s): s is string => !!s,
);

function run(shell: string, bin: string, line = installCommand("T", HOST)) {
  const r = Bun.spawnSync([shell, "-c", line], { env: { PATH: bin }, stdout: "pipe", stderr: "pipe" });
  return { code: r.exitCode, out: r.stdout.toString(), err: r.stderr.toString() };
}

describe("the install command", () => {
  test("says wget, else curl, else fail — then hands the token to sh as root", () => {
    expect(installCommand("T", HOST)).toBe(
      `(${WGET} || ${CURL} || echo "echo \\"${FAILED.trim()}\\" >&2; exit 1") 2>/dev/null | ` +
        `sudo OST_TOKEN=T sh -s -- --server ${HOST}`,
    );
  });

  test("plain http: says --insecure-http, or the installer refuses", () => {
    expect(installCommand("T", "http://ost-host.local:18080")).toEndWith(
      "sudo OST_TOKEN=T sh -s -- --server http://ost-host.local:18080 --insecure-http",
    );
  });

  for (const shell of shells) {
    describe(shell, () => {
      test("stock Debian or Ubuntu (wget, no curl): installs, and says nothing about curl", () => {
        const c = computer(["wget"]);
        const r = run(shell, c.bin);
        expect(r.out).toBe(RAN);
        expect(r.code).toBe(0);
        expect(r.err).toBe("");
        expect(c.calls()).toEqual([WGET]);
      });

      test("Arch (curl, no wget): installs with curl, and says nothing about wget", () => {
        const c = computer(["curl"]);
        const r = run(shell, c.bin);
        expect(r.out).toBe(RAN);
        expect(r.code).toBe(0);
        expect(r.err).toBe("");
        expect(c.calls()).toEqual([CURL]);
      });

      test("both (Fedora): one download", () => {
        const c = computer(["curl", "wget"]);
        expect(run(shell, c.bin).out).toBe(RAN);
        expect(c.calls()).toEqual([WGET]);
      });

      // Acceptance round 5: with wget there but the download failing (the
      // server down, a typo), the only word was "curl: command not found".
      // Every way the download can fail says one thing, names the server,
      // and exits non-zero — never a quiet exit 0.
      test("wget fails and there is no curl (stock Debian, server down): one clear message", () => {
        const c = computer(["wget"], null);
        const r = run(shell, c.bin);
        expect(r.code).toBe(1);
        expect(r.out).toBe("");
        expect(r.err).toBe(FAILED);
        expect(c.calls()).toEqual([WGET]);
      });

      test("neither downloader: the same one message", () => {
        const r = run(shell, computer([]).bin);
        expect(r.code).toBe(1);
        expect(r.out).toBe("");
        expect(r.err).toBe(FAILED);
      });

      test("both, and the server doesn't answer: tries both, then the one message", () => {
        const c = computer(["curl", "wget"], null);
        const r = run(shell, c.bin);
        expect(r.code).toBe(1);
        expect(r.out).toBe("");
        expect(r.err).toBe(FAILED);
        expect(c.calls()).toEqual([WGET, CURL]);
      });

      test("curl alone, and the server doesn't answer: the one message", () => {
        const c = computer(["curl"], null);
        const r = run(shell, c.bin);
        expect(r.code).toBe(1);
        expect(r.err).toBe(FAILED);
        expect(c.calls()).toEqual([CURL]);
      });

      test("the real installer, fetched with wget, starts and checks what it needs", () => {
        // It stops at the first check it can't pass here (not root; as root, no
        // sha256sum on this PATH) — having parsed whole.
        const r = run(shell, computer(["wget"], REAL).bin);
        expect(r.code).toBe(1);
        expect(r.err).toMatch(/ERROR: (must run as root|sha256sum is required)/);
      });
    });
  }
});
