// The confirm sheet's bookkeeping. What is *not* here matters as much as what
// is: nothing in this file decides whether a batch may run, what it costs, or
// which shots it names. The server counts and estimates; this only makes the
// answer legible. A copy of the estimator here would be the one way the sheet
// could promise something the batch does not do.
//
// Node's built-in runner, so this costs no dependency: `npm test`.
import test from "node:test";
import assert from "node:assert/strict";
import {
  formatCount,
  formatGpu,
  formatBytes,
  timeFromMinutes,
  minutesFromTime,
  formatWindow,
  fanoutSummary,
  estimateCaveats,
  batchStatusColor,
  batchProgress,
  selectionFromQuery,
  selectionShorthand,
  capsPayload,
} from "./batches.js";

// ── Counts ──

test("a count is separated so it can be read at a glance", () => {
  assert.equal(formatCount(12431), "12,431");
  assert.equal(formatCount(6658), "6,658");
  assert.equal(formatCount(0), "0");
});

test("a count with nothing behind it says so rather than NaN", () => {
  assert.equal(formatCount(null), "—");
  assert.equal(formatCount(undefined), "—");
  assert.equal(formatCount("nope"), "—");
});

// ── GPU time ──

test("GPU time climbs to the unit that still says something", () => {
  // 147,600 seconds is technically true and practically useless.
  assert.equal(formatGpu(25), "25 s");
  assert.equal(formatGpu(600), "10 min");
  assert.equal(formatGpu(3600 * 5), "5.0 h");
  assert.equal(formatGpu(3600 * 41), "41 h");
  assert.equal(formatGpu(3600 * 50), "2 d 2 h");
  assert.equal(formatGpu(3600 * 48), "2 d");
});

test("no GPU time reads as nothing", () => {
  assert.equal(formatGpu(0), "—");
  assert.equal(formatGpu(null), "—");
});

// ── Disk ──

test("disk is shown in the unit a person has a feel for", () => {
  assert.equal(formatBytes(512), "512 B");
  assert.equal(formatBytes(2 * 1024 ** 2), "2.0 MB");
  assert.equal(formatBytes(780 * 1024 ** 3), "780 GB");
  assert.equal(formatBytes(3.5 * 1024 ** 4), "3.5 TB");
});

test("no disk reads as nothing", () => {
  assert.equal(formatBytes(0), "—");
  assert.equal(formatBytes(undefined), "—");
});

// ── The window ──

test("minutes from midnight read as a clock", () => {
  assert.equal(timeFromMinutes(0), "00:00");
  assert.equal(timeFromMinutes(420), "07:00");
  assert.equal(timeFromMinutes(1320), "22:00");
});

test("a clock parses back to minutes from midnight", () => {
  assert.equal(minutesFromTime("00:00"), 0);
  assert.equal(minutesFromTime("07:00"), 420);
  assert.equal(minutesFromTime("7:30"), 450);
});

test("what is not a time is refused rather than guessed at", () => {
  assert.equal(minutesFromTime(""), null);
  assert.equal(minutesFromTime("25:00"), null);
  assert.equal(minutesFromTime("07:99"), null);
  assert.equal(minutesFromTime("overnight"), null);
});

test("a window reads as a range, including one that wraps midnight", () => {
  assert.equal(formatWindow(0, 420), "00:00–07:00");
  assert.equal(formatWindow(1320, 360), "22:00–06:00");
  assert.equal(formatWindow(null, 420), "");
});

// ── Fan-out ──

test("a line with no sweep shows no multiplier, because x1 is noise", () => {
  assert.equal(fanoutSummary([{ fanout: 1 }, { fanout: 1 }]), "");
  assert.equal(fanoutSummary([]), "");
  assert.equal(fanoutSummary(null), "");
});

test("every stage that multiplies is named", () => {
  assert.equal(fanoutSummary([{ fanout: 2 }]), "×2");
  assert.equal(fanoutSummary([{ fanout: 4 }, { fanout: 1 }, { fanout: 2 }]), "×4 · ×2");
});

// ── When the estimate is a ceiling ──

test("a line that holds makes its estimate an upper bound, and says so", () => {
  const caveats = estimateCaveats({ has_hold: true, guessed_stages: 0 });
  assert.equal(caveats.length, 1);
  assert.match(caveats[0], /holds for review/);
});

test("a stage that has never run here is called a guess, not a measurement", () => {
  const caveats = estimateCaveats({ has_hold: false, guessed_stages: 2 });
  assert.match(caveats[0], /2 stages have never run here/);
  assert.match(caveats[0], /guess/);
});

test("the singular is right, because a sheet that says '1 stages' is not read", () => {
  assert.match(estimateCaveats({ guessed_stages: 1 })[0], /1 stage has/);
});

test("everything measured and nothing holding needs no caveat", () => {
  assert.deepEqual(estimateCaveats({ has_hold: false, guessed_stages: 0 }), []);
  assert.deepEqual(estimateCaveats(null), []);
});

// ── The board ──

test("paused is coloured as waiting, not as a fault", () => {
  // A cap saying "not now" is not something going wrong.
  assert.equal(batchStatusColor("paused"), "var(--status-degraded)");
  assert.equal(batchStatusColor("running"), "var(--status-building)");
  assert.equal(batchStatusColor("completed"), "var(--status-ready)");
  assert.equal(batchStatusColor("stopped"), "var(--status-stopped)");
});

test("progress is measured against what was agreed at Send", () => {
  // Not against what the query says now: it is re-asked every tick and the
  // library moves, so a moving total would make the bar go backwards.
  assert.equal(
    batchProgress({
      matched_total: 100,
      skipped_total: 20,
      runs_completed: 40,
      runs_failed: 0,
      runs_cancelled: 0,
    }),
    50,
  );
});

test("a failed or cancelled run counts as done, because it is not coming back", () => {
  assert.equal(
    batchProgress({
      matched_total: 10,
      skipped_total: 0,
      runs_completed: 4,
      runs_failed: 3,
      runs_cancelled: 3,
    }),
    100,
  );
});

test("progress never goes past 100 or below 0", () => {
  assert.equal(batchProgress({ matched_total: 2, runs_completed: 9 }), 100);
  assert.equal(batchProgress({ matched_total: 0 }), 0);
  assert.equal(batchProgress(null), 0);
});

// ── The selection ──

test("an empty filter is left out rather than sent", () => {
  // `person_id=""` is not "no person" — it is a person whose id is the empty
  // string, and the backend would dutifully match nothing.
  assert.deepEqual(selectionFromQuery({ person_id: "", q: "  ", to: "1990" }), {
    kind: "query",
    query: { to: "1990" },
  });
});

test("what the selection keeps, it trims", () => {
  assert.deepEqual(selectionFromQuery({ q: "  grandma  " }), {
    kind: "query",
    query: { q: "grandma" },
  });
});

test("no filter at all is the whole library, and says so", () => {
  assert.deepEqual(selectionFromQuery({}), { kind: "query", query: {} });
  assert.equal(selectionShorthand({ kind: "query", query: {} }), "whole library");
});

test("a selection reads back as the sentence that produced it", () => {
  assert.equal(
    selectionShorthand({ kind: "query", query: { person_id: "p", to: "1990-01-01" } }),
    "person · –1990",
  );
  assert.equal(selectionShorthand({ kind: "ids", ids: ["a", "b"] }), "2 selected");
});

// ── Caps ──

test("a cap that was not set is left out, not sent as null", () => {
  assert.deepEqual(capsPayload({}), {});
});

test("a window goes whole or not at all", () => {
  assert.deepEqual(
    capsPayload({ windowEnabled: true, windowStart: "00:00", windowEnd: "07:00" }),
    { window_start_minute: 0, window_end_minute: 420 },
  );
  // Half a window paces nothing, and sending one half would look accepted.
  assert.deepEqual(
    capsPayload({ windowEnabled: true, windowStart: "00:00", windowEnd: "" }),
    {},
  );
});

test("a switched-off window sends nothing, however it was filled in", () => {
  assert.deepEqual(
    capsPayload({ windowEnabled: false, windowStart: "00:00", windowEnd: "07:00" }),
    {},
  );
});

test("the disk floor is typed in gigabytes and sent in bytes", () => {
  assert.deepEqual(capsPayload({ diskFloorGb: 50 }), {
    disk_floor_bytes: 50 * 1024 ** 3,
  });
});

test("the two caps that stop a mountain travel together", () => {
  assert.deepEqual(capsPayload({ dailyTaskCap: 400, maxOutstandingHolds: 200 }), {
    daily_task_cap: 400,
    max_outstanding_holds: 200,
  });
});
