/**
 * The Takes lane's rules, with no Vue anywhere near them.
 *
 * The lane is the screen a person actually spends their time on: generation is
 * cheap and deciding is not, and the number this has to hit is **two hundred
 * takes in ten minutes** — three seconds each, on a keyboard, without ever
 * reaching for the mouse. Two consequences shaped this file.
 *
 * **The keyboard is a pure function.** `keyAction` turns an event into an
 * intent and `reduce` turns an intent into the next state plus a list of
 * effects; nothing here touches the network, the DOM or a `ref`. That is what
 * lets the whole interaction model be tested by `node --test` — a verdict given
 * entirely from the keyboard is a sequence of calls in a test file, not a
 * browser somebody has to drive.
 *
 * **Rejecting arms, it does not delete.** `X` marks a take and the bytes go
 * when the verdict is sent, which is what lets the key cost one keystroke and
 * no dialog while still being reversible right up to the moment it is not — and
 * lets the footer print the megabytes the next Enter will free *before* it is
 * pressed. A confirmation nobody reads is not a safeguard; a number that is
 * always on screen is.
 *
 * What is deliberately not here: whether a verdict is allowed. `settle_verdict`
 * on the server decides that, and a second copy of the rule in JavaScript is
 * the one way this screen could refuse something the runtime would have
 * accepted, or promise something it will not.
 */

import { takeSeed } from "./lines.js";

// ===== What tells four takes apart =========================================

/**
 * Which parameters actually differ across the takes of one hold.
 *
 * A four-seed fan-out differs in exactly one key and a person is choosing
 * between four otherwise identical pictures, so printing the whole parameter
 * map on every card is noise and printing none of it is the bug FR5c's finisher
 * fixed. What is worth printing is the difference.
 *
 * Keys are `"<node_id>.<field>"`; the field is what a person reads, because the
 * node id is whatever the workflow's author happened to number the sampler.
 */
export function varyingKeys(takes) {
  const seen = new Map();
  for (const take of takes || []) {
    const params = take?.parameters;
    if (!params || typeof params !== "object") continue;
    for (const [key, value] of Object.entries(params)) {
      const at = seen.get(key) || new Set();
      at.add(JSON.stringify(value));
      seen.set(key, at);
    }
  }
  return [...seen.entries()]
    .filter(([, values]) => values.size > 1)
    .map(([key]) => key)
    .sort();
}

/** The field half of a `"<node_id>.<field>"` parameter key. */
function fieldOf(key) {
  const dot = key.indexOf(".");
  return dot < 0 ? key : key.slice(dot + 1);
}

/** The first eight characters of an id, which is how this repo prints one. */
export function shortId(id) {
  return String(id ?? "").slice(0, 8);
}

// ===== The batch a run came from ===========================================

/**
 * What to say about a run's batch, given whatever FR7's endpoint answered.
 *
 * Two things make this worth a function rather than an expression in the
 * template. The first is that the endpoint may not be there: FR7 ships
 * `GET /api/comfyui/batches`, and until that lands — or when it fails, or when
 * a batch has been deleted out from under a still-held run — the lane still has
 * a `batch_id` to draw. It falls back to the short id, which is what it drew
 * before there was anything better, rather than to nothing.
 *
 * The second is the pause. FR7's outstanding-hold cap stops a batch feeding
 * when more runs are held than it allows, and **the person in this lane is the
 * one who unblocks it** — they are looking at exactly the runs whose verdicts
 * bring the count down. A batch that is paused waiting on this screen and does
 * not say so is the single most useful sentence the lane could be printing and
 * is not.
 */
export function batchOf(sheet, batches = {}) {
  const id = sheet?.batch_id;
  if (!id) return null;
  const row = (batches && batches[id]) || null;
  const label = typeof row?.label === "string" && row.label.trim() ? row.label.trim() : null;
  return {
    id,
    // The name FR7 gives it, or the id it has always had. Never empty.
    label: label || shortId(id),
    /** Whether that label is a real name or a fallback, so the tag can say `batch` either way. */
    named: Boolean(label),
    status: row?.status || null,
    paused: row?.status === "paused",
    pausedReason: row?.paused_reason || null,
    // FR7 writes the sentence; repeating it here in different words is how two
    // screens come to disagree about why something stopped.
    pausedNote: row?.paused_note || null,
  };
}

/**
 * The sentence to print when a batch is paused, or `null`.
 *
 * FR7's own `paused_note` wherever there is one — never a paraphrase, and the
 * reason for that is not the obvious one. "One string so two screens cannot
 * disagree" is true but did not earn its place; what earned it is that when
 * this string grew from 62 characters to 111 it broke the layout of *both*
 * screens rendering it, and neither author noticed until one of them measured.
 * Sharing the string is what gave that bug a single place to surface. The cost
 * of forking copy is not maintenance, it is that a forked bug has nowhere to
 * show up.
 *
 * Every reason gets its note, including the ones no verdict here can lift.
 * The hazard was never "this pause has no sentence" — it was a sentence that
 * implies reviewing helps, and FR7's wording names what actually lifts each
 * one ("It picks up again then.", "Only freeing disk lifts this."). A still
 * lane with no explanation is the worse failure. The fallback below fires only
 * when there is no note at all, and then only for `holds`, which is the one
 * pause this screen is the cure for.
 */
export function batchNotice(batch) {
  if (!batch?.paused) return null;
  if (batch.pausedNote) return batch.pausedNote;
  if (batch.pausedReason === "holds") {
    return "This batch is paused until enough of these runs have a verdict.";
  }
  return null;
}

/**
 * The mono strip under one take's picture: what makes *this* one this one.
 *
 * The seed first, because it is the answer to the question asked most often and
 * `takeSeed` already matches the field against the same aliases the backend's
 * `ParamName::Seed` does. Then anything else that varied across the sheet —
 * steps, cfg, a frame count — because a fan-out is not always a seed sweep.
 *
 * And when nothing distinguishes them — a describe stage's takes carry no
 * parameters at all — the take's **position**, not a truncated id. Three
 * sentences labelled `RUN-C-TA`, `RUN-C-TA`, `RUN-C-TA` is the same failure as
 * four cards labelled with four indistinguishable hex strings, and a prefix of
 * an id is exactly the thing that collides when ids share one. A position
 * cannot collide, and it is what a person says out loud anyway.
 */
export function takeMarks(take, varying = [], index = null) {
  const marks = [];
  const seed = takeSeed(take);
  if (seed !== null) marks.push({ label: "seed", value: String(seed) });

  const params = take?.parameters || {};
  for (const key of varying) {
    const field = fieldOf(key).toLowerCase();
    if (seed !== null && (field === "seed" || field === "noise_seed" || field === "rand_seed")) {
      continue;
    }
    if (!(key in params)) continue;
    const value = params[key];
    if (value === null || typeof value === "object") continue;
    marks.push({ label: fieldOf(key), value: String(value) });
  }

  if (!marks.length) {
    marks.push(
      index === null
        ? { label: "", value: shortId(take?.task_id) }
        : { label: "take", value: String(index + 1) },
    );
  }
  return marks;
}

// ===== Bytes ===============================================================

/** Bytes as a person reads them, in the mono register the lane prints in. */
export function formatBytes(bytes) {
  const n = Number(bytes);
  if (!Number.isFinite(n) || n <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  let v = n;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  // One decimal below ten, none above: "1.4 GB", "412 MB". A screen that says
  // "412.0 MB" is a screen that has stopped reading like a schedule.
  return `${v < 10 && i > 0 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}

// ===== The state a lane is in ==============================================

/** A fresh lane over a page of held runs. */
export function initialState(sheets = []) {
  return {
    sheets,
    run: 0,
    take: 0,
    /** run_id -> task ids the reviewer marked worth continuing with. */
    kept: {},
    /** run_id -> task ids whose bytes go when the verdict is sent. */
    rejected: {},
    /** task_id -> 1..5, held locally so the card redraws before the PUT lands. */
    ratings: {},
    /** The original beside the takes, rather than the takes alone. */
    compare: false,
    /** The provenance panel: how this take was made, and how to make another. */
    provenance: false,
    /** `"cancel"` once cancel has been pressed but not confirmed. */
    armed: null,
    /** The next verdict goes to the whole batch. */
    bulk: false,
    help: false,
    /** The last thing that happened, for the status line. */
    said: "",
  };
}

export function currentSheet(state) {
  return state.sheets[state.run] || null;
}

export function currentTake(state) {
  const sheet = currentSheet(state);
  return sheet ? sheet.takes[state.take] || null : null;
}

const listFor = (map, runId) => map[runId] || [];

export function isKept(state, runId, taskId) {
  return listFor(state.kept, runId).includes(taskId);
}

export function isRejected(state, runId, taskId) {
  return listFor(state.rejected, runId).includes(taskId);
}

/**
 * What the next Enter will do, in the three numbers that matter.
 *
 * This is the safeguard. `reject` and the bytes behind it are on screen at all
 * times, so the key that frees them is never a surprise.
 */
export function verdictSummary(state) {
  const sheet = currentSheet(state);
  if (!sheet) return { keep: 0, reject: 0, pass: 0, bytes: 0 };
  const kept = listFor(state.kept, sheet.run_id);
  const rejected = listFor(state.rejected, sheet.run_id);
  const bytes = sheet.takes
    .filter((t) => rejected.includes(t.task_id))
    .reduce((sum, t) => sum + (Number(t.file_size) || 0), 0);
  return {
    keep: kept.length,
    reject: rejected.length,
    pass: sheet.takes.length - kept.length - rejected.length,
    bytes,
  };
}

// ===== The key map, as data ================================================

/**
 * Every key the lane answers to, in the order the help overlay lists them.
 *
 * One table, read by both the handler and the overlay, so a key that works and
 * a key that is documented cannot drift apart.
 */
export const KEY_MAP = [
  { keys: "1–5", does: "rate this take" },
  { keys: "0", does: "clear the rating" },
  { keys: "X", does: "reject — its bytes go when the verdict is sent" },
  { keys: "⏎", does: "keep this take and continue the run" },
  { keys: "⇧⏎", does: "keep it and stay, to keep two of four" },
  { keys: "space", does: "play / pause" },
  { keys: "P", does: "promote to the shot's main file" },
  { keys: "R", does: "regenerate — fresh seeds, nothing else changed" },
  { keys: "⌫ ⌫", does: "abandon this run (twice)" },
  { keys: "C", does: "compare against the original" },
  { keys: "I", does: "how this take was made" },
  { keys: "B", does: "aim the next verdict at the whole batch" },
  { keys: "← →", does: "move between takes" },
  { keys: "↑ ↓", does: "move between runs" },
  { keys: "?", does: "this list" },
];

/**
 * One key event, as an intent — or `null` when the lane does not answer to it.
 *
 * Typing into a field is never a shortcut, and neither is a browser shortcut: a
 * modifier other than shift means the key belongs to somebody else.
 */
export function keyAction(event, state = {}) {
  const target = event?.target;
  const tag = target?.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || target?.isContentEditable) return null;
  if (event.metaKey || event.ctrlKey || event.altKey) return null;

  const key = event.key;

  if (key === "Escape") return { type: "escape" };
  if (key === "?" || (key === "/" && event.shiftKey)) return { type: "help" };
  if (state.help) return { type: "escape" };

  if (key === "Enter") return { type: "keep", commit: !event.shiftKey };
  if (key === " " || key === "Spacebar") return { type: "play" };
  if (key === "ArrowLeft") return { type: "move", axis: "take", by: -1 };
  if (key === "ArrowRight") return { type: "move", axis: "take", by: 1 };
  if (key === "ArrowUp") return { type: "move", axis: "run", by: -1 };
  if (key === "ArrowDown") return { type: "move", axis: "run", by: 1 };
  if (key === "Backspace" || key === "Delete") return { type: "cancel" };

  if (/^[0-5]$/.test(key)) {
    return { type: "rate", rating: key === "0" ? null : Number(key) };
  }

  switch (key.toLowerCase()) {
    case "x":
      return { type: "reject" };
    case "p":
      return { type: "promote" };
    case "r":
      return { type: "regenerate" };
    case "c":
      return { type: "compare" };
    case "i":
      return { type: "provenance" };
    case "b":
      return { type: "bulk" };
    default:
      return null;
  }
}

// ===== And what an intent does =============================================

const clamp = (n, lo, hi) => Math.max(lo, Math.min(n, hi));

function without(list, id) {
  return (list || []).filter((x) => x !== id);
}

function withId(list, id) {
  return (list || []).includes(id) ? list : [...(list || []), id];
}

/** The verdict effect a commit sends, with everything armed on this run. */
function verdictEffect(state, sheet, verdict) {
  return {
    kind: "verdict",
    runId: sheet.run_id,
    verdict,
    keep: verdict === "continue" ? listFor(state.kept, sheet.run_id) : [],
    reject: listFor(state.rejected, sheet.run_id),
    scope: state.bulk && sheet.batch_id ? "batch" : "run",
  };
}

/**
 * The lane's whole interaction model: state in, state and effects out.
 *
 * Effects are descriptions, not calls — `{kind: "verdict"|"rate"|"promote"|
 * "play"}` — which is what keeps this testable and what keeps the component a
 * shell that performs them.
 */
export function reduce(state, action) {
  const effects = [];
  if (!action) return { state, effects };

  const sheet = currentSheet(state);
  const take = currentTake(state);
  const next = { ...state, said: "" };

  switch (action.type) {
    case "help":
      next.help = !state.help;
      return { state: next, effects };

    case "escape":
      // Unwound in the order somebody would expect to unwind it.
      if (state.help) next.help = false;
      else if (state.provenance) next.provenance = false;
      else if (state.armed) next.armed = null;
      else if (state.bulk) next.bulk = false;
      else if (state.compare) next.compare = false;
      return { state: next, effects };

    case "compare":
      next.compare = !state.compare;
      return { state: next, effects };

    case "provenance":
      next.provenance = !state.provenance;
      if (next.provenance && take?.output_file_id) {
        // "How did I make this, and can I make another?" — the manifest is the
        // answer and it is one request, asked only when somebody asks for it.
        effects.push({ kind: "provenance", fileId: take.output_file_id });
      }
      return { state: next, effects };

    case "bulk":
      if (!sheet?.batch_id) {
        next.said = "This run is not part of a batch.";
        return { state: next, effects };
      }
      next.bulk = !state.bulk;
      next.said = next.bulk
        ? "The next verdict goes to the whole batch. Cancelling takes is not stopping the batch."
        : "";
      return { state: next, effects };

    case "move": {
      next.armed = null;
      if (action.axis === "run") {
        next.run = clamp(state.run + action.by, 0, Math.max(0, state.sheets.length - 1));
        next.take = 0;
      } else if (sheet) {
        next.take = clamp(state.take + action.by, 0, Math.max(0, sheet.takes.length - 1));
      }
      return { state: next, effects };
    }

    case "rate": {
      if (!take) return { state: next, effects };
      if (!take.output_file_id) {
        // A describe stage's take is a sentence. There is nothing there to rate
        // one to five, and saying so beats a key that silently does nothing.
        next.said = "A written take has no picture to rate.";
        return { state: next, effects };
      }
      next.ratings = { ...state.ratings, [take.task_id]: action.rating };
      effects.push({ kind: "rate", fileId: take.output_file_id, rating: action.rating });
      return { state: next, effects };
    }

    case "reject": {
      if (!take || !sheet) return { state: next, effects };
      const armed = isRejected(state, sheet.run_id, take.task_id);
      next.rejected = {
        ...state.rejected,
        [sheet.run_id]: armed
          ? without(listFor(state.rejected, sheet.run_id), take.task_id)
          : withId(listFor(state.rejected, sheet.run_id), take.task_id),
      };
      // A take cannot be both. The server refuses that rather than guessing,
      // and the lane should never be able to ask.
      next.kept = {
        ...state.kept,
        [sheet.run_id]: without(listFor(state.kept, sheet.run_id), take.task_id),
      };
      next.take = clamp(state.take + (armed ? 0 : 1), 0, Math.max(0, sheet.takes.length - 1));
      return { state: next, effects };
    }

    case "keep": {
      if (!take || !sheet) return { state: next, effects };
      const kept = withId(listFor(state.kept, sheet.run_id), take.task_id);
      next.kept = { ...state.kept, [sheet.run_id]: kept };
      next.rejected = {
        ...state.rejected,
        [sheet.run_id]: without(listFor(state.rejected, sheet.run_id), take.task_id),
      };
      if (!action.commit) {
        // Keep and stay: the two-of-four case, one modifier away from the
        // one-keystroke path rather than in place of it.
        next.take = clamp(state.take + 1, 0, Math.max(0, sheet.takes.length - 1));
        return { state: next, effects };
      }
      effects.push(verdictEffect({ ...next }, sheet, "continue"));
      return { state: next, effects };
    }

    case "regenerate":
      if (!sheet) return { state: next, effects };
      effects.push(verdictEffect(state, sheet, "regenerate"));
      return { state: next, effects };

    case "cancel": {
      if (!sheet) return { state: next, effects };
      if (state.armed !== "cancel") {
        // Keeping and rejecting are reversible until the verdict is sent.
        // Abandoning a run is not, so it is the one key that asks twice.
        next.armed = "cancel";
        next.said = "Press again to abandon this run.";
        return { state: next, effects };
      }
      next.armed = null;
      effects.push(verdictEffect(state, sheet, "cancel"));
      return { state: next, effects };
    }

    case "promote": {
      if (!take) return { state: next, effects };
      if (!take.output_file_id) {
        next.said = "A written take is not a picture to promote.";
        return { state: next, effects };
      }
      if (take.is_main_file) {
        next.said = "Already the shot's main file.";
        return { state: next, effects };
      }
      effects.push({ kind: "promote", fileId: take.output_file_id, shotId: sheet?.shot_id });
      return { state: next, effects };
    }

    case "play":
      if (!take?.output_file_id) return { state: next, effects };
      effects.push({ kind: "play", taskId: take.task_id });
      return { state: next, effects };

    default:
      return { state, effects };
  }
}

/**
 * Drop the runs a verdict just settled, and land the cursor somewhere sensible.
 *
 * Sensible means *where the eye already is*: the next run takes the index the
 * decided one had, so a person holding Enter never has to look for the cursor.
 * At the end of the page it steps back rather than falling off.
 */
export function settle(state, runIds) {
  const gone = new Set(runIds);
  const sheets = state.sheets.filter((s) => !gone.has(s.run_id));
  const kept = { ...state.kept };
  const rejected = { ...state.rejected };
  for (const id of gone) {
    delete kept[id];
    delete rejected[id];
  }
  return {
    ...state,
    sheets,
    kept,
    rejected,
    run: clamp(state.run, 0, Math.max(0, sheets.length - 1)),
    take: 0,
    armed: null,
    bulk: false,
  };
}

/** How the lane counts itself: runs waiting, and takes across them. */
export function backlog(sheets) {
  return {
    runs: (sheets || []).length,
    takes: (sheets || []).reduce((n, s) => n + (s.takes?.length || 0), 0),
  };
}
