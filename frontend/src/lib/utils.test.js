// The console's own decisions about a workflow's inputs: which control a field
// wants, what type its value goes on the wire as, and how many runs a sweep is.
//
// Node's built-in runner, so this costs no dependency: `npm test`.
import test from "node:test";
import assert from "node:assert/strict";
import {
  controlKind,
  isTextInput,
  isParameterInput,
  kindLabel,
  numberBounds,
  parameterValue,
  parseValueList,
  randomSeed,
  runCount,
  formatDuration,
  stageOf,
  readinessColor,
  installedLabel,
  isDescribeWorkflow,
  slotKey,
  applyCompiledPrompt,
  mergeConstraints,
  MAX_SAFE_SEED,
} from "./utils.js";

/** One input as the workflow list endpoint serves it. */
const input = (widget, current_value, extra = {}) => ({
  node_id: "3",
  node_type: "KSampler",
  field_name: "steps",
  current_value,
  ...(widget ? { widget } : {}),
  ...extra,
});

const SEED = input({ kind: "seed", min: 0, max: 9223372036854775807 }, 156680208700286);
const STEPS = input({ kind: "int", default: 20, min: 1, max: 10000 }, 20);
const CFG = input({ kind: "float", default: 8.0, min: 0.0, max: 100.0, step: 0.1 }, 8.0);
const SWITCH = input({ kind: "boolean", default: false }, false);
const CKPT = input({ kind: "combo", choices: ["a.safetensors", "b.safetensors"] }, "a.safetensors");
const PROMPT = input({ kind: "text", multiline: true }, "a photograph");

test("every widget kind gets the control it wants", () => {
  assert.equal(controlKind(SEED), "seed");
  assert.equal(controlKind(STEPS), "int");
  assert.equal(controlKind(CFG), "float");
  assert.equal(controlKind(SWITCH), "boolean");
  assert.equal(controlKind(CKPT), "combo");
  assert.equal(controlKind(PROMPT), "textarea");
  assert.equal(controlKind(input({ kind: "text", multiline: false }, "x")), "text");
});

test("FR3's fallback still renders as the text box it always was", () => {
  // No widget: the heuristics found it and it is a string by construction.
  assert.equal(controlKind(input(null, "custom pack, not installed here")), "textarea");
  assert.ok(isTextInput(input(null, "anything")));
  assert.equal(kindLabel(input(null, "anything")), "text?");
});

test("what cannot be typed into is not offered", () => {
  // The shot is the image input; the source picker fills it.
  assert.equal(controlKind({ node_type: "LoadImage", current_value: "example.png" }), null);
  // A widget kind from a newer backend is skipped rather than guessed at.
  assert.equal(controlKind(input({ kind: "nebulous" }, 1)), null);
  // And a non-string with no widget is not a text box.
  assert.equal(controlKind(input(null, 42)), null);
});

test("the two override channels do not overlap", () => {
  for (const i of [SEED, STEPS, CFG, SWITCH, CKPT]) {
    assert.ok(isParameterInput(i), `${i.field_name} should be typed`);
    assert.ok(!isTextInput(i));
  }
  for (const i of [PROMPT, input(null, "fallback")]) {
    assert.ok(isTextInput(i));
    assert.ok(!isParameterInput(i));
  }
});

test("a value goes on the wire as the JSON type its field holds", () => {
  // Browsers hand back strings from every input element.
  assert.equal(parameterValue(STEPS, "28"), 28);
  assert.equal(parameterValue(CFG, "6.5"), 6.5);
  assert.equal(parameterValue(SEED, "4242"), 4242);
  assert.equal(parameterValue(SWITCH, true), true);
  assert.equal(parameterValue(CKPT, "b.safetensors"), "b.safetensors");
  // An int control never emits a float, whatever a spinner does.
  assert.equal(parameterValue(STEPS, "28.9"), 28);
  // Nothing readable falls back to the node's own default rather than NaN.
  assert.equal(parameterValue(STEPS, "twenty"), 20);
});

test("opening the dialog and pressing enhance runs the author's own graph", () => {
  assert.equal(parameterValue(SEED), 156680208700286);
  assert.equal(parameterValue(STEPS), 20);
  assert.equal(parameterValue(CFG), 8);
  assert.equal(parameterValue(CKPT), "a.safetensors");
  assert.equal(parameterValue(SWITCH), false);
});

test("a number control is bounded by the node's own range", () => {
  assert.deepEqual(numberBounds(STEPS), { min: 1, max: 10000, step: 1 });
  assert.deepEqual(numberBounds(CFG), { min: 0, max: 100, step: 0.1 });
  // A seed's range is clamped to what a JSON parser can hold exactly.
  const seed = numberBounds(SEED);
  assert.equal(seed.min, 0);
  assert.equal(seed.max, MAX_SAFE_SEED);
});

test("a re-rolled seed is one the console can display and the server can store", () => {
  for (let i = 0; i < 200; i++) {
    const seed = randomSeed(SEED);
    assert.ok(Number.isSafeInteger(seed), `${seed} is not exact`);
    assert.ok(seed >= 0 && seed <= MAX_SAFE_SEED);
  }
});

test("a swept list is read as the field's type, or refused", () => {
  assert.deepEqual(parseValueList("4, 6, 8", CFG), [4, 6, 8]);
  assert.deepEqual(parseValueList("10,20 , 30", STEPS), [10, 20, 30]);
  assert.deepEqual(parseValueList("a.safetensors, b.safetensors", CKPT), [
    "a.safetensors",
    "b.safetensors",
  ]);
  // Half-typed, and nonsense, are both "not yet a sweep".
  assert.equal(parseValueList("", CFG), null);
  assert.deepEqual(parseValueList("4, ", CFG), [4]);
  assert.equal(parseValueList("four, six", CFG), null);
  // A name this server does not have is refused rather than queued.
  assert.equal(parseValueList("c.safetensors", CKPT), null);
  // Unless the stored list was capped, in which case it cannot say.
  const truncated = input({ kind: "combo", choices: ["a.safetensors"], truncated: true }, "a.safetensors");
  assert.deepEqual(parseValueList("z.safetensors", truncated), ["z.safetensors"]);
});

test("the run count is the product of the axes", () => {
  assert.equal(runCount({}), 1);
  assert.equal(runCount({ "3.seed": { count: 4, mode: "random" } }), 4);
  assert.equal(runCount({ "3.cfg": { values: [4, 6, 8] } }), 3);
  assert.equal(
    runCount({ "3.seed": { count: 4, mode: "increment" }, "3.cfg": { values: [4, 6, 8] } }),
    12,
  );
  // The short spellings the API also accepts count the same.
  assert.equal(runCount({ "3.seed": 4, "3.cfg": [4, 6, 8] }), 12);
});

test("a run's clock reads as a schedule, not as a number of seconds", () => {
  assert.equal(formatDuration(192), "00:03:12");
  assert.equal(formatDuration(0), "00:00:00");
  assert.equal(formatDuration(3661), "01:01:01");
  // A run whose start the server could not parse still gets a clock-shaped gap.
  assert.equal(formatDuration(null), "--:--:--");
});

test("a run says which stage of how many, counting from one", () => {
  // Stage index 1 of a four-stage line is the second stage.
  assert.equal(stageOf({ current_stage: 1, stage_count: 4, status: "running" }), "2/4");
  assert.equal(stageOf({ current_stage: 0, stage_count: 4, status: "running" }), "1/4");
  // A finished run reads 4/4, never 5/4.
  assert.equal(stageOf({ current_stage: 4, stage_count: 4, status: "completed" }), "4/4");
  // A failure names the stage it stopped at, so it can be resumed from there.
  assert.equal(stageOf({ current_stage: 1, stage_count: 4, status: "failed" }), "2/4");
  // A lone workflow is a one-stage run, and reads as one.
  assert.equal(stageOf({ current_stage: 0, stage_count: 1, status: "running" }), "1/1");
});

test("a template's readiness is painted from the status palette", () => {
  assert.equal(readinessColor("ready"), "var(--status-ready)");
  assert.equal(readinessColor("missing"), "var(--status-error)");
  assert.equal(readinessColor("degraded"), "var(--status-degraded)");
  // Not knowing is neutral, not red: the catalogue could not be read, which is
  // not evidence that anything is wrong.
  assert.equal(readinessColor("unchecked"), "var(--status-stopped)");
  assert.equal(readinessColor(undefined), "var(--status-stopped)");
});

test("a template says whether it is installed, and whether it is still ours to update", () => {
  assert.equal(installedLabel({ version: 1 }), "NOT INSTALLED");
  assert.equal(
    installedLabel({ version: 2, installed: { version: 2, line_exists: true, customised: false } }),
    "INSTALLED v2",
  );
  // The one that matters: an edited template is the user's, for good.
  assert.equal(
    installedLabel({ version: 2, installed: { version: 1, line_exists: true, customised: true } }),
    "EDITED \u00b7 NOT UPDATED",
  );
  assert.equal(
    installedLabel({ version: 1, installed: { version: 1, line_exists: false, customised: false } }),
    "LINE DELETED",
  );
});

// --- FR9: the prompt is compiled, and putting it in must match the backend ---

/** A generation workflow the way the list endpoint serves one. */
const CLIP_WORKFLOW = {
  id: "wf-gen",
  contract: {
    accepts: "image",
    produces: "video",
    slots: [
      { name: "positive", node_id: "6", field: "text" },
      { name: "negative", node_id: "7", field: "text" },
    ],
  },
};

const DESCRIBE_WORKFLOW = {
  id: "wf-describe",
  contract: { accepts: "image", produces: "text", slots: [{ name: "positive", node_id: "2", field: "prompt" }] },
};

test("a describe workflow is the one whose contract hands on text", () => {
  assert.equal(isDescribeWorkflow(DESCRIBE_WORKFLOW), true);
  assert.equal(isDescribeWorkflow(CLIP_WORKFLOW), false);
  // A workflow imported before contracts existed says nothing, not "yes".
  assert.equal(isDescribeWorkflow({ id: "wf-old" }), false);
});

test("a prompt slot is addressed by the key the backend substitutes on", () => {
  assert.equal(slotKey(CLIP_WORKFLOW, "positive"), "6.text");
  assert.equal(slotKey(CLIP_WORKFLOW, "negative"), "7.text");
  assert.equal(slotKey(CLIP_WORKFLOW, "scene"), null);
});

test("using a compiled prompt replaces the positive and grows the negative", () => {
  const before = { "6.text": "a leftover default", "7.text": "blurry, watermark" };
  const after = applyCompiledPrompt(CLIP_WORKFLOW, before, {
    positive: "Anna on a jetty at dusk.",
    negative: "change face, add people",
  });
  assert.equal(after["6.text"], "Anna on a jetty at dusk.");
  assert.equal(after["7.text"], "blurry, watermark, change face, add people");
  // And nothing was written into the map the dialog is still holding.
  assert.equal(before["6.text"], "a leftover default");
});

test("a constraint already in the negative prompt is not repeated", () => {
  assert.equal(mergeConstraints("blurry, Change Face", "change face, warp hands"),
    "blurry, Change Face, warp hands");
  assert.equal(mergeConstraints("", "change face"), "change face");
  assert.equal(mergeConstraints("blurry.", ""), "blurry");
});

test("a workflow with no negative slot still takes the prompt", () => {
  const wf = { contract: { slots: [{ name: "positive", node_id: "6", field: "text" }] } };
  const after = applyCompiledPrompt(wf, {}, { positive: "a cat.", negative: "change face" });
  assert.deepEqual(after, { "6.text": "a cat." });
});
