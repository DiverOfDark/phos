import test from "node:test";
import assert from "node:assert/strict";

import { cn } from "./utils.js";

test("cn merges class lists and lets the last tailwind utility win", () => {
  assert.equal(cn("p-2", "p-4"), "p-4");
  assert.equal(cn("text-ink", false && "hidden", "border-line"), "text-ink border-line");
});
