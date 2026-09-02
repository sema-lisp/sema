import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

// The e2e suite exercises copies of the web-demo apps under
// e2e/fixtures/scripts. The demo under examples/web-demo is what users read,
// so the two must stay identical; otherwise the tests cover an app nobody
// ships and the shipped app has no coverage.
const here = import.meta.dirname;
const fixtures = resolve(here, "../e2e/fixtures/scripts");
const demo = resolve(here, "../../../examples/web-demo");

describe("web-demo fixtures", () => {
  for (const name of ["board.sema", "chat.sema", "chat-widget.sema"]) {
    it(`${name} matches examples/web-demo/${name}`, () => {
      expect(readFileSync(resolve(fixtures, name), "utf8")).toBe(
        readFileSync(resolve(demo, name), "utf8"),
      );
    });
  }
});
