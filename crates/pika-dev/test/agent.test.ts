import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { getSessionMethod } from "../src/agent.js";

describe("agent session method binding", () => {
  it("binds the method receiver to the raw session object", () => {
    const session = {
      name: "pika",
      greet(this: { name: string }, suffix: string): string {
        return `${this.name}-${suffix}`;
      },
    } as Record<string, unknown>;

    const greet = getSessionMethod<[string], string>(session, "greet");
    assert.ok(greet);
    assert.equal(greet("ok"), "pika-ok");
  });

  it("returns undefined for missing methods", () => {
    const session = {} as Record<string, unknown>;
    const missing = getSessionMethod<[], void>(session, "nope");
    assert.equal(missing, undefined);
  });
});

