// The Takes lane, driven entirely from the keyboard.
//
// The lane's whole claim is that two hundred takes can be reviewed in ten
// minutes without touching a mouse, so the thing worth testing is the sequence
// of keystrokes and the request it ends in. `keyAction` and `reduce` are pure,
// which is what lets that be a test file rather than a browser somebody drives.
//
// Node's built-in runner, so this costs no dependency: `npm test`.
import test from "node:test";
import assert from "node:assert/strict";

import {
  KEY_MAP,
  backlog,
  currentTake,
  formatBytes,
  initialState,
  isKept,
  isRejected,
  keyAction,
  reduce,
  settle,
  takeMarks,
  varyingKeys,
  verdictSummary,
} from "./takes.js";

// ===== Fixtures ============================================================

/** One take of a fan-out: a clip, a hundred and forty megabytes, one seed. */
const take = (i, extra = {}) => ({
  task_id: `take-${i}`,
  output_file_id: `file-${i}`,
  thumbnail_url: `/api/files/file-${i}/thumbnail`,
  file_url: `/api/files/file-${i}`,
  text_output: null,
  mime_type: "video/mp4",
  file_size: 140 * 1024 * 1024,
  rating: null,
  is_main_file: false,
  // Node 17, not node 3: the sampler is numbered by whoever drew the workflow.
  parameters: { "17.noise_seed": 1000 + i, "17.steps": 20, "17.cfg": 7.5 },
  completed_at: "2026-08-31 09:00:00",
  is_source: false,
  ...extra,
});

/** A held run: a ×4 extend, one upscale below it, out of a batch. */
const sheet = (id, takes = 4, extra = {}) => ({
  run_id: id,
  shot_id: `shot-${id}`,
  label: "Extend then upscale",
  stage_idx: 1,
  stage_count: 3,
  stage_label: "Extend Clip",
  created_at: "2026-08-31 08:00:00",
  batch_id: "batch-a",
  source_file_id: "file-original",
  source_thumbnail_url: "/api/files/file-original/thumbnail",
  main_file_id: "file-original",
  takes: Array.from({ length: takes }, (_, i) => take(i)),
  fanouts: [1],
  tasks_per_take: 1,
  ...extra,
});

/** A key event as the window hands one over. */
const press = (key, mods = {}) => ({
  key,
  target: { tagName: "DIV" },
  shiftKey: false,
  metaKey: false,
  ctrlKey: false,
  altKey: false,
  ...mods,
});

/** Play a whole sequence of keys, collecting every effect it produced. */
function type(state, keys) {
  const effects = [];
  for (const entry of keys) {
    const event = typeof entry === "string" ? press(entry) : press(entry[0], entry[1]);
    const result = reduce(state, keyAction(event, state));
    state = result.state;
    effects.push(...result.effects);
  }
  return { state, effects };
}

// ===== The one-keystroke path ==============================================

test("one Enter on the take you want is the whole verdict", () => {
  // The number the lane exists to hit. Arrow to the good one, press Enter, and
  // the run continues with it: two keystrokes for a four-take fan-out.
  const start = initialState([sheet("run-1")]);
  const { effects } = type(start, ["ArrowRight", "ArrowRight", "Enter"]);
  assert.equal(effects.length, 1);
  assert.deepEqual(effects[0], {
    kind: "verdict",
    runId: "run-1",
    verdict: "continue",
    keep: ["take-2"],
    reject: [],
    scope: "run",
  });
});

test("shift-Enter keeps a take without committing, so two of four is possible", () => {
  // A hold is a fan-out point as much as a filter: keep two and both walk the
  // rest of the line. One modifier away from the fast path, not in place of it.
  const start = initialState([sheet("run-1")]);
  const first = type(start, [["Enter", { shiftKey: true }]]);
  assert.deepEqual(first.effects, [], "keep and stay sends nothing");
  assert.ok(isKept(first.state, "run-1", "take-0"));
  assert.equal(first.state.take, 1, "and moves on, because you are still choosing");

  const second = type(first.state, ["Enter"]);
  assert.deepEqual(second.effects[0].keep, ["take-0", "take-1"]);
});

test("the cursor stops at the ends rather than wrapping into the wrong run", () => {
  // Wrapping is how somebody reviewing at speed gives a verdict on a run they
  // were not looking at.
  const start = initialState([sheet("run-1", 3), sheet("run-2", 2)]);
  const left = type(start, ["ArrowLeft", "ArrowLeft"]);
  assert.equal(left.state.take, 0);
  assert.equal(left.state.run, 0);

  const right = type(start, ["ArrowRight", "ArrowRight", "ArrowRight", "ArrowRight"]);
  assert.equal(right.state.take, 2, "three takes, so index two is the last");

  const up = type(start, ["ArrowUp"]);
  assert.equal(up.state.run, 0);
  const down = type(start, ["ArrowDown", "ArrowDown"]);
  assert.equal(down.state.run, 1);
  assert.equal(down.state.take, 0, "and a new run starts at its first take");
});

// ===== Reject ==============================================================

test("X arms a rejection and says how many bytes the next Enter will free", () => {
  // The safeguard is a number on screen, not a dialog: `X` costs one keystroke
  // and nothing is deleted until the verdict goes.
  const start = initialState([sheet("run-1")]);
  const { state, effects } = type(start, ["x", "x", "x"]);
  assert.deepEqual(effects, [], "arming deletes nothing on its own");
  assert.deepEqual(state.rejected["run-1"], ["take-0", "take-1", "take-2"]);

  const summary = verdictSummary(state);
  assert.equal(summary.reject, 3);
  assert.equal(summary.pass, 1);
  assert.equal(summary.bytes, 420 * 1024 * 1024);
  assert.equal(formatBytes(summary.bytes), "420 MB");
});

test("X on an armed take disarms it, right up until the verdict goes", () => {
  const start = initialState([sheet("run-1")]);
  const armed = type(start, ["x"]);
  assert.ok(isRejected(armed.state, "run-1", "take-0"));
  // Rejecting moves on, so come back before pressing it again.
  const disarmed = type(armed.state, ["ArrowLeft", "x"]);
  assert.equal(isRejected(disarmed.state, "run-1", "take-0"), false);
  assert.equal(verdictSummary(disarmed.state).bytes, 0);
});

test("reject three, keep one, and the verdict carries both lists", () => {
  // What the lane actually sends after a four-take fan-out: one continues, three
  // have their bytes freed, and the runtime is asked exactly once.
  const start = initialState([sheet("run-1")]);
  const { effects } = type(start, ["x", "x", "x", "Enter"]);
  assert.equal(effects.length, 1);
  assert.deepEqual(effects[0].keep, ["take-3"]);
  assert.deepEqual(effects[0].reject, ["take-0", "take-1", "take-2"]);
});

test("a take cannot be both kept and rejected, whichever key came last", () => {
  // The server refuses that rather than guessing which of the two words
  // deletes the file. The lane should never be able to ask.
  const start = initialState([sheet("run-1")]);
  const kept = type(start, [["Enter", { shiftKey: true }], "ArrowLeft", "x"]);
  assert.equal(isKept(kept.state, "run-1", "take-0"), false);
  assert.ok(isRejected(kept.state, "run-1", "take-0"));

  const rejected = type(start, ["x", "ArrowLeft", ["Enter", { shiftKey: true }]]);
  assert.equal(isRejected(rejected.state, "run-1", "take-0"), false);
  assert.ok(isKept(rejected.state, "run-1", "take-0"));
});

// ===== Rating, promote, play ===============================================

test("1 to 5 rates the take under the cursor and 0 clears it", () => {
  const start = initialState([sheet("run-1")]);
  const rated = type(start, ["4"]);
  assert.deepEqual(rated.effects[0], { kind: "rate", fileId: "file-0", rating: 4 });
  assert.equal(rated.state.ratings["take-0"], 4);

  const cleared = type(rated.state, ["0"]);
  assert.deepEqual(cleared.effects[0], { kind: "rate", fileId: "file-0", rating: null });
  assert.equal(cleared.state.ratings["take-0"], null, "not rated is not rated zero");
});

test("a describe stage's take is a sentence, and says so instead of doing nothing", () => {
  // No picture to rate, none to promote, no bytes to free. A key that silently
  // does nothing is worse than one that explains itself.
  const written = sheet("run-1", 1, {
    takes: [take(0, { output_file_id: null, file_url: null, file_size: null, text_output: "A jetty at dusk." })],
  });
  const start = initialState([written]);

  const rated = type(start, ["3"]);
  assert.deepEqual(rated.effects, []);
  assert.match(rated.state.said, /no picture to rate/i);

  const promoted = type(start, ["p"]);
  assert.deepEqual(promoted.effects, []);
  assert.match(promoted.state.said, /not a picture/i);

  const played = type(start, [" "]);
  assert.deepEqual(played.effects, [], "and nothing to play");
});

test("P promotes the take under the cursor, and does not ask twice", () => {
  const start = initialState([sheet("run-1")]);
  const { effects } = type(start, ["ArrowRight", "p"]);
  assert.deepEqual(effects[0], { kind: "promote", fileId: "file-1", shotId: "shot-run-1" });

  const already = initialState([
    sheet("run-1", 1, { takes: [take(0, { is_main_file: true })] }),
  ]);
  const again = type(already, ["p"]);
  assert.deepEqual(again.effects, []);
  assert.match(again.state.said, /already/i);
});

test("space plays the take under the cursor", () => {
  const start = initialState([sheet("run-1")]);
  const { effects } = type(start, ["ArrowRight", " "]);
  assert.deepEqual(effects[0], { kind: "play", taskId: "take-1" });
});

// ===== The two verdicts that are not continue ==============================

test("R regenerates without naming any take, because seeds are all that change", () => {
  const start = initialState([sheet("run-1")]);
  const { effects } = type(start, [["Enter", { shiftKey: true }], "r"]);
  assert.equal(effects[0].verdict, "regenerate");
  assert.deepEqual(effects[0].keep, [], "a regenerate is about the stage, not a selection");
});

test("abandoning a run is the one key that asks twice", () => {
  // Keeping and rejecting are reversible until the verdict goes. Abandoning is
  // not, so it is armed rather than immediate — the same two-press guard the
  // Shots lane uses for delete.
  const start = initialState([sheet("run-1")]);
  const armed = type(start, ["Backspace"]);
  assert.deepEqual(armed.effects, []);
  assert.equal(armed.state.armed, "cancel");
  assert.match(armed.state.said, /press again/i);

  const done = type(armed.state, ["Backspace"]);
  assert.equal(done.effects[0].verdict, "cancel");
  assert.equal(done.state.armed, null);
});

test("moving the cursor disarms a half-pressed abandon", () => {
  const start = initialState([sheet("run-1")]);
  const { state } = type(start, ["Backspace", "ArrowRight", "Backspace"]);
  assert.equal(state.armed, "cancel", "the second press re-arms rather than firing");
});

test("a rejection still travels with an abandon, because the disk is the point", () => {
  const start = initialState([sheet("run-1")]);
  const { effects } = type(start, ["x", "x", "x", "x", "Backspace", "Backspace"]);
  assert.equal(effects[0].verdict, "cancel");
  assert.equal(effects[0].reject.length, 4);
});

// ===== Bulk ================================================================

test("B aims the next verdict at the whole batch, and says what that does not do", () => {
  const start = initialState([sheet("run-1")]);
  const aimed = type(start, ["b"]);
  assert.equal(aimed.state.bulk, true);
  assert.match(aimed.state.said, /not stopping the batch/i);

  const { effects } = type(aimed.state, ["Enter"]);
  assert.equal(effects[0].scope, "batch");
});

test("a run that belongs to no batch cannot be bulk-decided", () => {
  // Until FR7 lands, that is every run. A lane that offered it anyway would be
  // offering a verdict the server quietly narrows.
  const start = initialState([sheet("run-1", 4, { batch_id: null })]);
  const aimed = type(start, ["b"]);
  assert.equal(aimed.state.bulk, false);
  assert.match(aimed.state.said, /not part of a batch/i);
  assert.equal(type(aimed.state, ["Enter"]).effects[0].scope, "run");
});

test("bulk is aimed one verdict at a time, not left switched on", () => {
  // Deciding four hundred runs by accident is the failure mode; a mode that
  // stays on across verdicts is how it happens.
  const start = initialState([sheet("run-1"), sheet("run-2")]);
  const aimed = type(start, ["b"]);
  const after = settle(aimed.state, ["run-1"]);
  assert.equal(after.bulk, false);
});

// ===== Settling and the cursor =============================================

test("a settled run leaves the page and the cursor stays where the eye is", () => {
  const start = initialState([sheet("run-1"), sheet("run-2"), sheet("run-3")]);
  const moved = { ...start, run: 1 };
  const after = settle(moved, ["run-2"]);
  assert.deepEqual(after.sheets.map((s) => s.run_id), ["run-1", "run-3"]);
  assert.equal(after.run, 1, "the next run takes the index the decided one had");
  assert.equal(after.take, 0);
});

test("settling the last run steps back rather than falling off the end", () => {
  const start = { ...initialState([sheet("run-1"), sheet("run-2")]), run: 1 };
  const after = settle(start, ["run-2"]);
  assert.equal(after.run, 0);
  const empty = settle(after, ["run-1"]);
  assert.equal(empty.sheets.length, 0);
  assert.equal(empty.run, 0, "and an empty lane has a cursor that is still a number");
});

test("a bulk verdict settles every run it touched, not only the one decided on", () => {
  const start = initialState([sheet("run-1"), sheet("run-2"), sheet("run-3")]);
  const after = settle(start, ["run-1", "run-3"]);
  assert.deepEqual(after.sheets.map((s) => s.run_id), ["run-2"]);
});

test("armed rejections do not survive their run leaving the page", () => {
  const start = initialState([sheet("run-1"), sheet("run-2")]);
  const armed = type(start, ["x", "x"]);
  const after = settle(armed.state, ["run-1"]);
  assert.equal(after.rejected["run-1"], undefined);
});

// ===== What is not a shortcut ==============================================

test("typing into a field is never a shortcut", () => {
  // The lane listens on the window, so a note or a search box would otherwise
  // reject a take on the letter x.
  for (const tagName of ["INPUT", "TEXTAREA"]) {
    assert.equal(keyAction({ ...press("x"), target: { tagName } }), null);
  }
  assert.equal(
    keyAction({ ...press("x"), target: { tagName: "DIV", isContentEditable: true } }),
    null,
  );
});

test("a browser shortcut belongs to the browser", () => {
  for (const mods of [{ metaKey: true }, { ctrlKey: true }, { altKey: true }]) {
    assert.equal(keyAction(press("r", mods)), null);
  }
});

test("escape unwinds one thing at a time, in the order somebody expects", () => {
  let state = { ...initialState([sheet("run-1")]), compare: true };
  state = type(state, ["b"]).state;
  state = type(state, ["Backspace"]).state;
  state = type(state, ["?"]).state;
  assert.equal(state.help, true);

  state = type(state, ["Escape"]).state;
  assert.equal(state.help, false);
  state = type(state, ["Escape"]).state;
  assert.equal(state.armed, null);
  state = type(state, ["Escape"]).state;
  assert.equal(state.bulk, false);
  state = type(state, ["Escape"]).state;
  assert.equal(state.compare, false);
});

test("every key the help overlay lists is a key the lane answers to", () => {
  // One table read by both, so a documented key and a working key cannot drift.
  assert.ok(KEY_MAP.length >= 12);
  const state = initialState([sheet("run-1")]);
  for (const key of ["1", "0", "x", "Enter", " ", "p", "r", "Backspace", "c", "b", "ArrowLeft", "ArrowUp", "?"]) {
    assert.notEqual(keyAction(press(key), state), null, `${key} does nothing`);
  }
});

// ===== Telling four takes apart ============================================

test("only the parameters that actually differ are worth printing", () => {
  // Four seeds, identical everything else: the card should say the seed and not
  // recite steps and cfg four times.
  const takes = sheet("run-1").takes;
  assert.deepEqual(varyingKeys(takes), ["17.noise_seed"]);
  assert.deepEqual(takeMarks(takes[2], varyingKeys(takes)), [
    { label: "seed", value: "1002" },
  ]);
});

test("a sweep over something other than a seed is printed by its field name", () => {
  const takes = [
    take(0, { parameters: { "17.noise_seed": 7, "17.steps": 20 } }),
    take(1, { parameters: { "17.noise_seed": 7, "17.steps": 40 } }),
  ];
  const varying = varyingKeys(takes);
  assert.deepEqual(varying, ["17.steps"]);
  assert.deepEqual(takeMarks(takes[1], varying), [
    { label: "seed", value: "7" },
    { label: "steps", value: "40" },
  ]);
});

test("a take with nothing to tell it apart falls back to an id, not to nothing", () => {
  // A describe stage carries no seed. "seed undefined" on four cards is the bug
  // this whole strip exists to avoid.
  const written = take(0, { parameters: {} });
  assert.deepEqual(takeMarks(written, []), [{ label: "", value: "take-0" }]);
});

test("the seed is found whatever the workflow numbered its sampler", () => {
  // Node 3 is ComfyUI's example graph and almost nobody else's.
  const t = take(0, { parameters: { "412.seed": 99 } });
  assert.deepEqual(takeMarks(t, []), [{ label: "seed", value: "99" }]);
});

// ===== Small things a schedule board has to get right ======================

test("bytes read like a schedule rather than a spreadsheet", () => {
  assert.equal(formatBytes(0), "0 B");
  assert.equal(formatBytes(null), "0 B");
  assert.equal(formatBytes(940), "940 B");
  assert.equal(formatBytes(412 * 1024 * 1024), "412 MB");
  assert.equal(formatBytes(1.4 * 1024 ** 3), "1.4 GB");
});

test("the lane counts runs and takes separately, because they are different backlogs", () => {
  assert.deepEqual(backlog([sheet("a", 4), sheet("b", 2)]), { runs: 2, takes: 6 });
  assert.deepEqual(backlog([]), { runs: 0, takes: 0 });
  assert.deepEqual(backlog(null), { runs: 0, takes: 0 });
});

test("an empty lane answers every key without throwing", () => {
  // The state after the last verdict lands, which is where a queue screen
  // usually breaks.
  const empty = initialState([]);
  assert.equal(currentTake(empty), null);
  for (const key of ["Enter", "x", "p", "r", "1", " ", "Backspace", "ArrowRight", "b"]) {
    const { effects } = type(empty, [key]);
    assert.deepEqual(effects, [], `${key} did something on an empty lane`);
  }
});
