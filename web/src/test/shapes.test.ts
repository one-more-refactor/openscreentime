// The console's types and its mock, held against what the server really sends.
//
// Acceptance round 4: the person page addressed everyone as `undefined` on a
// real server. `types.ts` declared `FamilyChild.account_id`, only the mock
// provided it, the server never sent it — and every console test passed on
// the mock. `server-shapes.json` is the shape of each response the person
// page reads, recorded by the server's own test (`server/src/tests_shapes.rs`)
// from a seeded household through the real handlers. Here:
//
//   1. every field `types.ts` declares as always there is really sent, with a
//      JSON type the declaration allows (a `string` is never `null`);
//   2. the mock sends nothing the server doesn't — so a page built and tested
//      on the mock can't lean on a field the real server lacks.
//
// When the server changes a response its test fails until the file is
// rewritten (`OST_WRITE_SHAPES=1 cargo test shapes`); this test then says what
// the console has to follow.
import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import ts from "typescript";
import shapes from "./server-shapes.json";
import { mockEarnRequests, mockEvents, mockFamily, mockWhere } from "../mock";

type Shape = string | Shape[] | { [k: string]: Shape };

const typesTs = ts.createSourceFile(
  "types.ts",
  readFileSync(new URL("../types.ts", import.meta.url), "utf8"),
  ts.ScriptTarget.Latest,
  true,
);
const decls = new Map<string, ts.InterfaceDeclaration | ts.TypeAliasDeclaration>();
for (const s of typesTs.statements) {
  if (ts.isInterfaceDeclaration(s) || ts.isTypeAliasDeclaration(s)) decls.set(s.name.text, s);
}

/** A type reference's name (works on the test's own synthesized roots too). */
function refName(t: ts.TypeReferenceNode): string {
  return ts.isIdentifier(t.typeName) ? t.typeName.text : t.typeName.right.text;
}

interface Prop {
  name: string;
  optional: boolean;
  type: ts.TypeNode | undefined;
}

function propsOf(members: ts.NodeArray<ts.TypeElement>): Prop[] {
  return members.filter(ts.isPropertySignature).map((m) => ({
    name: m.name.getText(typesTs),
    optional: !!m.questionToken,
    type: m.type,
  }));
}

/** An interface's properties, inherited ones included. */
function interfaceProps(d: ts.InterfaceDeclaration): Prop[] {
  const own = propsOf(d.members);
  const inherited = (d.heritageClauses ?? []).flatMap((h) =>
    h.types.flatMap((t) => {
      const base = decls.get(t.expression.getText(typesTs));
      return base && ts.isInterfaceDeclaration(base) ? interfaceProps(base) : [];
    }),
  );
  return [...inherited, ...own];
}

/** Which JSON types a declared type admits, or null for "anything". */
function leafKinds(t: ts.TypeNode | undefined): Set<string> | null {
  if (!t) return null;
  if (ts.isParenthesizedTypeNode(t)) return leafKinds(t.type);
  if (ts.isUnionTypeNode(t)) {
    const all = new Set<string>();
    for (const x of t.types) {
      const k = leafKinds(x);
      if (k === null) return null;
      k.forEach((v) => all.add(v));
    }
    return all;
  }
  if (ts.isLiteralTypeNode(t)) {
    if (t.literal.kind === ts.SyntaxKind.NullKeyword) return new Set(["null"]);
    if (ts.isStringLiteral(t.literal)) return new Set(["string"]);
    if (ts.isNumericLiteral(t.literal)) return new Set(["number"]);
    return new Set(["boolean"]);
  }
  switch (t.kind) {
    case ts.SyntaxKind.StringKeyword:
      return new Set(["string"]);
    case ts.SyntaxKind.NumberKeyword:
      return new Set(["number"]);
    case ts.SyntaxKind.BooleanKeyword:
      return new Set(["boolean"]);
    case ts.SyntaxKind.NullKeyword:
      return new Set(["null"]);
  }
  if (ts.isArrayTypeNode(t)) return new Set(["array"]);
  if (ts.isTypeLiteralNode(t)) return new Set(["object"]);
  if (ts.isTypeReferenceNode(t)) {
    const name = refName(t);
    const d = decls.get(name);
    if (!d) return name === "Record" ? new Set(["object"]) : null;
    if (ts.isInterfaceDeclaration(d)) return new Set(["object"]);
    return leafKinds(d.type);
  }
  return null;
}

/** The object or array part of a declared type (`X | null` → X). */
function structural(t: ts.TypeNode | undefined): ts.TypeNode | undefined {
  if (!t) return undefined;
  if (ts.isParenthesizedTypeNode(t)) return structural(t.type);
  if (ts.isUnionTypeNode(t)) {
    const parts = t.types.map(structural).filter((x): x is ts.TypeNode => !!x);
    return parts.length === 1 ? parts[0] : undefined;
  }
  if (ts.isArrayTypeNode(t) || ts.isTypeLiteralNode(t)) return t;
  if (ts.isTypeReferenceNode(t)) {
    const d = decls.get(refName(t));
    if (!d) return undefined;
    if (ts.isInterfaceDeclaration(d)) return t;
    return structural(d.type);
  }
  return undefined;
}

function kindOf(s: Shape): string {
  return Array.isArray(s) ? "array" : typeof s === "object" ? "object" : s;
}

/** Everything wrong with `server` as a value of type `t`, as path lines. */
function check(path: string, t: ts.TypeNode | undefined, server: Shape, out: string[]): void {
  const kinds = leafKinds(t);
  const k = kindOf(server);
  if (kinds && !kinds.has(k)) {
    out.push(`${path}: the server sends ${k}, the console declares ${[...kinds].join(" | ")}`);
    return;
  }
  const s = structural(t);
  if (!s || k === "null") return;
  if (ts.isArrayTypeNode(s)) {
    if (Array.isArray(server) && server.length > 0) check(`${path}[]`, s.elementType, server[0], out);
    return;
  }
  let props: Prop[] = [];
  if (ts.isTypeLiteralNode(s)) props = propsOf(s.members);
  else if (ts.isTypeReferenceNode(s)) {
    const d = decls.get(refName(s));
    if (d && ts.isInterfaceDeclaration(d)) props = interfaceProps(d);
  }
  const obj = server as Record<string, Shape>;
  for (const p of props) {
    if (!(p.name in obj)) {
      if (!p.optional) out.push(`${path}.${p.name}: declared by the console, never sent by the server`);
      continue;
    }
    check(`${path}.${p.name}`, p.type, obj[p.name], out);
  }
}

function typeRef(name: string): ts.TypeNode {
  expect(decls.has(name)).toBe(true);
  return ts.factory.createTypeReferenceNode(name);
}

/** The same shape the server test records (see tests_shapes.rs). */
function shapeOf(v: unknown): Shape {
  if (v === null || v === undefined) return "null";
  if (Array.isArray(v)) {
    const els = v.map(shapeOf);
    return els.length ? [els.reduce(mergeShapes)] : [];
  }
  if (typeof v === "object") {
    const out: Record<string, Shape> = {};
    for (const [k, x] of Object.entries(v)) {
      if (x === undefined) continue;
      out[k] = k === "payload" && x && typeof x === "object" && !Array.isArray(x) ? "object" : shapeOf(x);
    }
    return out;
  }
  return typeof v;
}

function mergeShapes(a: Shape, b: Shape): Shape {
  if (a === "null") return b;
  if (b === "null") return a;
  if (Array.isArray(a) && Array.isArray(b)) {
    const all = [...a, ...b];
    return all.length ? [all.reduce(mergeShapes)] : [];
  }
  if (typeof a === "object" && typeof b === "object" && !Array.isArray(a) && !Array.isArray(b)) {
    const out = { ...a };
    for (const [k, v] of Object.entries(b)) out[k] = k in out ? mergeShapes(out[k], v) : v;
    return out;
  }
  return a;
}

/** Keys the mock sends that the server doesn't, as path lines. */
function extra(path: string, mock: Shape, server: Shape, out: string[]): void {
  if (server === "null" || mock === "null") return;
  if (Array.isArray(mock) && Array.isArray(server)) {
    if (mock.length && server.length) extra(`${path}[]`, mock[0], server[0], out);
    return;
  }
  if (typeof mock === "object" && !Array.isArray(mock) && typeof server === "object" && !Array.isArray(server)) {
    for (const [k, v] of Object.entries(mock)) {
      if (!(k in server)) out.push(`${path}.${k}: the mock sends it, the server doesn't`);
      else extra(`${path}.${k}`, v, server[k], out);
    }
  }
}

const server = shapes as unknown as Record<string, Record<string, Shape>>;

describe("the console's types match what the server sends", () => {
  test("GET /api/family", () => {
    const out: string[] = [];
    check("family", typeRef("FamilyResponse"), server["GET /api/family"], out);
    expect(out).toEqual([]);
  });

  test("a person is addressed by the id the server sends", () => {
    // The field every per-person call uses — never undefined again.
    const child = (server["GET /api/family"].children as Shape[])[0] as Record<string, Shape>;
    expect(child.account_id).toBe("string");
    expect(child.key).toBe("string");
  });

  test("GET /api/usage/where", () => {
    const out: string[] = [];
    check("where", typeRef("WhereData"), server["GET /api/usage/where"], out);
    expect(out).toEqual([]);
  });

  test("GET /api/events", () => {
    const out: string[] = [];
    const events = server["GET /api/events"].events as Shape[];
    expect(events.length).toBe(1);
    check("events[]", typeRef("Event"), events[0], out);
    expect(out).toEqual([]);
  });
});

describe("the mock sends nothing the server doesn't", () => {
  test("family", () => {
    const out: string[] = [];
    extra("family", shapeOf(mockFamily()), server["GET /api/family"], out);
    extra("family.requests", shapeOf(mockEarnRequests), server["GET /api/family"].requests, out);
    expect(out).toEqual([]);
  });

  test("where the time went", () => {
    const out: string[] = [];
    extra("where", shapeOf(mockWhere()), server["GET /api/usage/where"], out);
    expect(out).toEqual([]);
  });

  test("events", () => {
    const out: string[] = [];
    extra("events", shapeOf({ events: mockEvents }), server["GET /api/events"], out);
    expect(out).toEqual([]);
  });

  test("the checks catch a drift like round 4's", () => {
    // What the server sent before: no account_id.
    const family = structuredClone(server["GET /api/family"]) as Record<string, Shape>;
    const child = (family.children as Shape[])[0] as Record<string, Shape>;
    delete child.account_id;
    const out: string[] = [];
    check("family", typeRef("FamilyResponse"), family, out);
    expect(out).toEqual(["family.children[].account_id: declared by the console, never sent by the server"]);
    const more: string[] = [];
    extra("family", shapeOf(mockFamily()), family, more);
    expect(more).toContain("family.children[].account_id: the mock sends it, the server doesn't");
  });
});
