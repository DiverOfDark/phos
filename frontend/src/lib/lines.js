/**
 * The line editor's own bookkeeping — and deliberately none of its rules.
 *
 * Whether a workflow may follow another one is a question this file never
 * answers. The editor sends the line it is holding to the server, which runs
 * its own `validate_chain` over each candidate; a second copy of that rule in
 * JavaScript is the one way this screen could be wrong in a way nobody
 * notices — the picker would offer a stage the dispatcher then refuses, four
 * hours into a run — and it would have to be kept in step with rules it does
 * not know about, such as a describe stage being transparent to the media
 * flow.
 *
 * So what lives here is everything that is *not* the rule. What to ask about
 * a given slot. What a connector says out loud. Which of the three
 * dispositions a setting is in. How a reorder rearranges an array, and what
 * the whole thing looks like as a request body.
 */

// ===== Which slot is being filled ==========================================

/**
 * What the picker is asked, for one position of the line being edited.
 *
 * The *whole draft* goes over, not the two types either side of the slot. It
 * would be easy to send `after: <what stage 2 produces>` — and wrong, because
 * whether a stage fits is a question about the line rather than about one
 * join: a stage that makes no file is transparent to what flows past it, so
 * the thing arriving at position 3 is not necessarily what stage 2 produced.
 * Sending the line and letting the server run its own validator over each
 * candidate is the only shape that cannot fall behind that rule.
 *
 * The two modes are genuinely different, and the difference is easy to get
 * wrong: *inserting* at index 2 pushes the current stage 2 down, so the new
 * stage has to feed it; *replacing* stage 2 removes it, so the new stage has
 * to feed stage 3 instead.
 */
export function pickerRequest(stages, index, mode = "insert") {
  return {
    stages: (stages || []).map((s) => s.workflow_id),
    at: Math.max(0, Math.min(index, (stages || []).length)),
    mode: mode === "replace" ? "replace" : "insert",
  };
}

// ===== What a connector says ===============================================

/** The source modes a connector can offer, in the words a person reads. */
export const MODE_WORDS = {
  whole_video: "whole video",
  first_frame: "first frame",
  last_frame: "last frame",
  at_time: "t = …",
  keyframe: "keyframe",
};

/** Split a stored source mode into the button that is lit and its number. */
export function parseSourceMode(mode) {
  const text = String(mode ?? "");
  const [key, argument] = text.includes(":") ? text.split(":", 2) : [text, null];
  const n = Number(argument);
  return {
    key: key || null,
    ms: key === "at_time" && Number.isFinite(n) ? n : 0,
    index: key === "keyframe" && Number.isFinite(n) ? n : 0,
  };
}

/** The string the backend stores, or `null` for "let the graph decide". */
export function sourceMode(key, { ms = 0, index = 0 } = {}) {
  if (!key) return null;
  if (key === "at_time") return `at_time:${Math.max(0, Math.trunc(ms) || 0)}`;
  if (key === "keyframe") return `keyframe:${Math.max(0, Math.trunc(index) || 0)}`;
  return key;
}

/**
 * What one join reads as: `image`, `whole video`, `t = 1.5s`.
 *
 * The handoff comes from the server, which resolved it the same way the
 * dispatcher will. A still is said as what it is rather than as "first frame",
 * because "the first frame of a JPEG" is not a thing anybody chose.
 */
export function handoffLabel(handoff) {
  if (!handoff) return "";
  if (handoff.carries !== "video") return handoff.carries;
  const { key, ms, index } = parseSourceMode(handoff.resolved);
  if (key === "at_time") return `t = ${formatSeconds(ms)}`;
  if (key === "keyframe") return `keyframe ${index}`;
  return MODE_WORDS[key] || handoff.resolved || "";
}

/** `1500` → `1.5s`, `90000` → `90s`. Trailing zeroes are noise on a label. */
export function formatSeconds(ms) {
  const seconds = Math.max(0, Number(ms) || 0) / 1000;
  return `${Number(seconds.toFixed(2))}s`;
}

// ===== Which disposition a setting is in ===================================

/**
 * `pinned`, `varied` or `exposed` — the three things a line can say about one
 * of a stage's settings.
 *
 * A fourth, `compiled` (written by a describe stage), is FR9's. It will be a
 * fourth branch here and a fourth tag in the row; nothing else about this
 * function has to change to make room for it.
 *
 * `varied` is checked first. The two are mutually exclusive and the backend
 * refuses a stage that claims both, but if a row somehow held both, the sweep
 * is the one that changes how many runs happen and so the one worth showing.
 */
export function dispositionOf(stage, key) {
  if (stage?.vary && key in stage.vary) return "varied";
  if ((stage?.exposed || []).includes(key)) return "exposed";
  return "pinned";
}

/** The word the row shows for a disposition. */
export const DISPOSITION_WORDS = {
  pinned: "pinned",
  exposed: "asked",
  varied: "varied",
};

/**
 * Put one key into one disposition, taking it out of the others.
 *
 * Returns the new `{ vary, exposed }` pair rather than mutating, so a Vue model
 * assignment is one statement and the old value is still there to compare to.
 */
export function setDisposition(stage, key, disposition) {
  const vary = { ...(stage?.vary || {}) };
  const exposed = (stage?.exposed || []).filter((k) => k !== key);
  delete vary[key];
  if (disposition === "exposed") exposed.push(key);
  return { vary, exposed };
}

// ===== Rearranging ==========================================================

/**
 * A stage moved from one position to another.
 *
 * A reorder is a whole new stage list, because that is what `PUT` takes — the
 * unique index on `(line_id, stage_idx)` is what keeps a line linear, and
 * rewriting it wholesale is the only way to move a row without a moment where
 * two stages claim one position.
 */
export function reorder(stages, from, to) {
  const list = [...(stages || [])];
  if (from < 0 || from >= list.length || to < 0 || to >= list.length || from === to) {
    return list;
  }
  const [moved] = list.splice(from, 1);
  list.splice(to, 0, moved);
  return list;
}

/** The whole line as `POST`/`PUT`/`validate` want it. */
export function toPayload(line) {
  return {
    name: String(line?.name ?? "").trim(),
    description: line?.description || null,
    stages: (line?.stages || []).map((stage) => ({
      workflow_id: stage.workflow_id,
      text_overrides: stage.text_overrides || {},
      parameters: stage.parameters || {},
      vary: stage.vary || {},
      source_mode: stage.source_mode ?? null,
      keep_output: !!stage.keep_output,
      exposed: stage.exposed || [],
    })),
  };
}

/**
 * The types a line reads as, end to end: `image → video`.
 *
 * The whole reason a chain is worth drawing rather than listing — what goes in
 * one end and what comes out the other is the fact that decides whether it can
 * be run against a given shot at all.
 */
export function typeTrack(line) {
  const stages = line?.stages || [];
  if (!stages.length) return "";
  return `${stages[0].accepts} → ${stages[stages.length - 1].produces}`;
}

/** How many of a stage's settings the line leaves to whoever sends it. */
export function askedCount(line) {
  return (line?.stages || []).reduce((total, s) => total + (s.exposed || []).length, 0);
}
