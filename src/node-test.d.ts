// The few `node:test` and `node:assert` signatures the `*.test.ts` files use, so that `tsc`
// type-checks the tests (fixtures included) without adding `@types/node`.

declare module "node:test" {
  export function describe(name: string, fn: () => void): void;
  export function it(name: string, fn: () => void | Promise<void>): void;
}

declare module "node:assert/strict" {
  const assert: {
    equal<T>(actual: unknown, expected: T, message?: string): asserts actual is T;
    deepEqual<T>(actual: unknown, expected: T, message?: string): asserts actual is T;
  };
  export default assert;
}

// Solo per i test che confrontano un contratto con una costante del backend.
declare module "node:fs" {
  export function readFileSync(path: URL, encoding: "utf8"): string;
}
