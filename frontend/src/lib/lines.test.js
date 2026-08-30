// The line editor's own bookkeeping. What is *not* here is as important as what
// is: whether one stage may follow another is never decided in JavaScript. The
// editor sends the line it is holding and the server runs its own validator
// over each candidate — a copy of that rule here would be the one way this
// screen could be wrong without anybody noticing.
//
// Node's built-in runner, so this costs no dependency: `npm test`.
import test from "node:test";
import assert from "node:assert/strict";
import {
  pickerRequest,
  parseSourceMode,
  sourceMode,
  handoffLabel,
  formatSeconds,
  dispositionOf,
  setDisposition,
  reorder,
  toPayload,
  typeTrack,
  askedCount,
  continuationCost,
  heldLabel,
} from "./lines.js";

/** One stage, as `GET /api/comfyui/lines/{id}` serves it. */
const stage = (name, accepts, produces, extra = {}) => ({
  workflow_id: `wf-${name}`,
  workflow_name: name,
  accepts,
  produces,
  keep_output: false,
  hold_for_review: false,
  source_mode: null,
  exposed: [],
  text_overrides: {},
  parameters: {},
  vary: {},
  ...extra,
});

/** photo → 5s clip → interpolate → 4K, the line the whole feature is for. */
const restore4k = () => [
  stage("Photo to 5s Clip", "image", "video"),
  stage("Interpolate 60fps", "video", "video"),
  stage("Upscale 4K", "video", "video"),
];

test("the picker is asked about the line, not about one join of it", () => {
  const stages = restore4k();
  // The whole draft goes over, so the server can answer with its own validator
  // rather than with a rule this file would have to keep in step.
  assert.deepEqual(pickerRequest(stages, 3), {
    stages: ["wf-Photo to 5s Clip", "wf-Interpolate 60fps", "wf-Upscale 4K"],
    at: 3,
    mode: "insert",
  });
  assert.deepEqual(pickerRequest([], 0), { stages: [], at: 0, mode: "insert" });
});

test("inserting and replacing are asked as different questions", () => {
  const stages = restore4k();
  assert.equal(pickerRequest(stages, 1, "replace").mode, "replace");
  assert.equal(pickerRequest(stages, 1).mode, "insert");
  assert.equal(pickerRequest(stages, 1, "nonsense").mode, "insert", "and nothing else is a mode");
  // A position past the end is the end, not a hole.
  assert.equal(pickerRequest(stages, 99).at, 3);
  assert.equal(pickerRequest(stages, -4).at, 0);
});

test("a source mode round-trips through the buttons that set it", () => {
  assert.deepEqual(parseSourceMode("whole_video"), { key: "whole_video", ms: 0, index: 0 });
  assert.deepEqual(parseSourceMode("at_time:1500"), { key: "at_time", ms: 1500, index: 0 });
  assert.deepEqual(parseSourceMode("keyframe:3"), { key: "keyframe", ms: 0, index: 3 });
  assert.equal(parseSourceMode(null).key, null);

  assert.equal(sourceMode("last_frame"), "last_frame");
  assert.equal(sourceMode("at_time", { ms: 1500 }), "at_time:1500");
  assert.equal(sourceMode("keyframe", { index: 3 }), "keyframe:3");
  assert.equal(sourceMode(null), null, "nothing chosen lets the graph decide");
  // Nonsense typed into the number box does not become a nonsense mode.
  assert.equal(sourceMode("at_time", { ms: -4 }), "at_time:0");
  assert.equal(sourceMode("at_time", { ms: "abc" }), "at_time:0");
});

test("a connector states the handoff in words, not in field names", () => {
  assert.equal(handoffLabel(null), "", "the first stage has no join above it");
  assert.equal(handoffLabel({ carries: "image", resolved: "first_frame" }), "image");
  assert.equal(handoffLabel({ carries: "video", resolved: "whole_video" }), "whole video");
  assert.equal(handoffLabel({ carries: "video", resolved: "last_frame" }), "last frame");
  assert.equal(handoffLabel({ carries: "video", resolved: "at_time:1500" }), "t = 1.5s");
  assert.equal(handoffLabel({ carries: "video", resolved: "keyframe:2" }), "keyframe 2");
  assert.equal(formatSeconds(90000), "90s");
  assert.equal(formatSeconds(250), "0.25s");
});

test("every setting is in exactly one of the three dispositions", () => {
  const pinned = stage("Upscale", "video", "video", { parameters: { "3.seed": 42 } });
  assert.equal(dispositionOf(pinned, "3.seed"), "pinned");
  assert.equal(dispositionOf(pinned, "3.cfg"), "pinned", "unset is the author's value, still pinned");

  const varied = stage("Upscale", "video", "video", { vary: { "3.seed": { count: 4 } } });
  assert.equal(dispositionOf(varied, "3.seed"), "varied");

  const asked = stage("Upscale", "video", "video", { exposed: ["6.text"] });
  assert.equal(dispositionOf(asked, "6.text"), "exposed");
});

test("choosing a disposition takes the key out of the other two", () => {
  const s = stage("Upscale", "video", "video", {
    vary: { "3.seed": { count: 4 }, "3.cfg": { values: [4, 6] } },
    exposed: ["6.text"],
  });

  // Asked: the sweep goes, the other sweep stays.
  const asked = setDisposition(s, "3.seed", "exposed");
  assert.deepEqual(asked.exposed, ["6.text", "3.seed"]);
  assert.deepEqual(Object.keys(asked.vary), ["3.cfg"]);

  // Pinned: out of both.
  const pinned = setDisposition({ ...s, exposed: ["3.seed"] }, "3.seed", "pinned");
  assert.deepEqual(pinned.exposed, []);
  assert.ok(!("3.seed" in pinned.vary));

  // And nothing is mutated: the caller still has the value to compare against.
  assert.deepEqual(s.exposed, ["6.text"]);
  assert.deepEqual(Object.keys(s.vary), ["3.seed", "3.cfg"]);
});

test("a reorder is a whole new stage list", () => {
  const names = (list) => list.map((s) => s.workflow_name);
  const stages = restore4k();
  assert.deepEqual(names(reorder(stages, 2, 0)), [
    "Upscale 4K",
    "Photo to 5s Clip",
    "Interpolate 60fps",
  ]);
  assert.deepEqual(names(reorder(stages, 0, 1)), [
    "Interpolate 60fps",
    "Photo to 5s Clip",
    "Upscale 4K",
  ]);
  // The original is untouched, so cancelling a drag costs nothing.
  assert.deepEqual(names(stages), ["Photo to 5s Clip", "Interpolate 60fps", "Upscale 4K"]);
  // Out of range, or nowhere, is a no-op rather than a hole in the list.
  assert.deepEqual(names(reorder(stages, 1, 1)), names(stages));
  assert.deepEqual(names(reorder(stages, 0, 9)), names(stages));
  assert.deepEqual(names(reorder(stages, -1, 0)), names(stages));
});

test("the editor's state goes back as the body the endpoint takes", () => {
  const line = {
    name: "  4K Restore  ",
    description: "",
    stages: [
      stage("Photo to 5s Clip", "image", "video", {
        text_overrides: { "6.text": "a winter street" },
        parameters: { "3.steps": 28 },
        exposed: ["6.text"],
      }),
      stage("Interpolate 60fps", "video", "video", {
        source_mode: "whole_video",
        keep_output: true,
      }),
    ],
  };
  const body = toPayload(line);
  assert.equal(body.name, "4K Restore");
  assert.equal(body.description, null);
  assert.equal(body.stages.length, 2);
  assert.deepEqual(body.stages[0], {
    workflow_id: "wf-Photo to 5s Clip",
    text_overrides: { "6.text": "a winter street" },
    parameters: { "3.steps": 28 },
    vary: {},
    source_mode: null,
    keep_output: false,
    exposed: ["6.text"],
    hold_for_review: false,
  });
  assert.equal(body.stages[1].source_mode, "whole_video");
  assert.equal(body.stages[1].keep_output, true);
  // Nothing but the fields the API declares: the read carries `accepts`,
  // `produces` and `handoff`, and sending those back would be sending the
  // server its own derivation.
  assert.deepEqual(Object.keys(body.stages[1]).sort(), [
    "exposed",
    "hold_for_review",
    "keep_output",
    "parameters",
    "source_mode",
    "text_overrides",
    "vary",
    "workflow_id",
  ]);
});

test("a hold point travels with the stage that asks for it", () => {
  const body = toPayload({
    name: "Extend",
    stages: [
      stage("Extend Clip", "video", "video", { hold_for_review: true }),
      stage("Upscale 4K", "video", "video"),
    ],
  });
  assert.equal(body.stages[0].hold_for_review, true);
  assert.equal(body.stages[1].hold_for_review, false);
  // And a stage read back from a library that predates holds is not holding.
  assert.equal(toPayload({ stages: [{ workflow_id: "wf" }] }).stages[0].hold_for_review, false);
});

test("keeping two of four does not halve the bill", () => {
  // A hold is a fan-out point as much as a filter: the estimate the board shows
  // is per kept take, so it goes up as boxes are ticked.
  assert.equal(continuationCost(2, 1), 2);
  assert.equal(continuationCost(2, 4), 8);
  assert.equal(continuationCost(0, 4), 0, "nothing kept, nothing queued");
  assert.equal(continuationCost(3, undefined), 0);
});

test("a held run says how many takes it is waiting on", () => {
  assert.equal(heldLabel({ held_takes: 4 }), "held · 4 takes");
  assert.equal(heldLabel({ held_takes: 1 }), "held · 1 take");
  // Before the count has loaded, or on a run holding nothing, it is just the
  // status — never `held · 0 takes`, which reads as a bug.
  assert.equal(heldLabel({ held_takes: 0 }), "held");
  assert.equal(heldLabel({}), "held");
});

test("a line reads as what goes in one end and what comes out the other", () => {
  assert.equal(typeTrack({ stages: restore4k() }), "image → video");
  assert.equal(typeTrack({ stages: [] }), "");
  assert.equal(
    askedCount({ stages: [stage("A", "image", "image", { exposed: ["6.text", "3.seed"] }), stage("B", "image", "image")] }),
    2,
  );
});
