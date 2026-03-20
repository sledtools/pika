import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { SendThrottle } from "./daemon-client.js";

describe("SendThrottle", () => {
  it("continues running queued sends after a failure", async () => {
    const throttle = new SendThrottle(0);
    let calls = 0;

    await assert.rejects(
      () =>
        throttle.enqueue(async () => {
          calls += 1;
          throw new Error("boom");
        }),
      /boom/,
    );

    await throttle.enqueue(async () => {
      calls += 1;
    });

    assert.equal(calls, 2);
  });
});
