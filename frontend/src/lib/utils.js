import { clsx } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs) {
  return twMerge(clsx(inputs));
}

/**
 * Is this detected workflow input one the console offers as a text box?
 *
 * Text — and anything the catalogue could not type, which is a string by
 * construction — rides in `text_overrides`, a string→string map. Everything
 * else rides in `parameters`, where a seed stays an integer and a checkpoint
 * stays the exact string ComfyUI listed. See `isParameterInput`.
 */
export function isTextInput(input) {
  const kind = controlKind(input);
  return kind === "text" || kind === "textarea";
}

/**
 * Which control this input wants, or `null` for one that is not editable.
 *
 * `null` covers the source image (the shot *is* the input), a wired socket, and
 * any widget kind a newer backend knows about and this build does not — better
 * a missing row than a control that writes nonsense into the graph.
 */
export function controlKind(input) {
  if (!input || input.node_type === "LoadImage") return null;
  const widget = input.widget;
  // No widget means the fallback heuristics found it and nothing is known
  // beyond "it holds a string" — which is every input Phos surfaced before
  // `/object_info` was read, and must keep working when it cannot be.
  if (!widget) return typeof input.current_value === "string" ? "textarea" : null;
  switch (widget.kind) {
    case "text":
      return widget.multiline ? "textarea" : "text";
    case "int":
      return "int";
    case "float":
      return "float";
    case "seed":
      return "seed";
    case "boolean":
      return "boolean";
    case "combo":
      return "combo";
    default:
      return null;
  }
}

/** Does this input belong in the typed `parameters` map rather than in text? */
export function isParameterInput(input) {
  return ["int", "float", "seed", "boolean", "combo"].includes(controlKind(input));
}

/** The short uppercase tag the row shows for what kind of field this is. */
export function kindLabel(input) {
  const widget = input?.widget;
  switch (controlKind(input)) {
    case "seed":
      return "seed";
    case "int":
      return "int";
    case "float":
      return "float";
    case "boolean":
      return "switch";
    case "combo":
      return "enum";
    case "text":
    case "textarea":
      return widget ? "text" : "text?";
    default:
      return "";
  }
}

/**
 * The largest seed the console will offer or draw: 2^53 − 1.
 *
 * ComfyUI's seed widgets go to 2^63 − 1, but JavaScript cannot hold that
 * exactly — a seed the console cannot display is worse than a smaller space,
 * and the backend draws inside the same bound.
 */
export const MAX_SAFE_SEED = Number.MAX_SAFE_INTEGER;

/** `{ min, max, step }` for a number-ish input, ready for an `<input>`. */
export function numberBounds(input) {
  const kind = controlKind(input);
  const widget = input?.widget || {};
  if (kind === "seed") {
    return {
      min: clampSeed(widget.min ?? 0),
      max: clampSeed(widget.max ?? MAX_SAFE_SEED),
      step: 1,
    };
  }
  return {
    min: widget.min ?? null,
    max: widget.max ?? null,
    step: widget.step ?? (kind === "int" ? 1 : "any"),
  };
}

function clampSeed(n) {
  const value = Number(n);
  if (!Number.isFinite(value)) return 0;
  return Math.min(Math.max(Math.trunc(value), 0), MAX_SAFE_SEED);
}

/** A fresh seed, drawn the same way and inside the same range as the server's. */
export function randomSeed(input) {
  const { min, max } = numberBounds(input);
  const span = Math.max(1, max - min + 1);
  return min + Math.floor(Math.random() * Math.min(span, MAX_SAFE_SEED));
}

/**
 * This input's value as the JSON type its field holds.
 *
 * Starts from what the graph already carries, so opening the dialog and
 * pressing Enhance runs the workflow exactly as its author saved it.
 */
export function parameterValue(input, raw) {
  const value = raw === undefined ? input?.current_value : raw;
  switch (controlKind(input)) {
    case "seed":
    case "int": {
      const n = Math.trunc(Number(value));
      return Number.isFinite(n) ? n : (input?.widget?.default ?? 0);
    }
    case "float": {
      const n = Number(value);
      return Number.isFinite(n) ? n : (input?.widget?.default ?? 0);
    }
    case "boolean":
      if (typeof value === "string") return value === "true";
      return Boolean(value);
    case "combo":
      return String(value ?? input?.widget?.default ?? "");
    default:
      return value;
  }
}

/** Key a stored override or parameter by, matching what the backend expects. */
export function inputKey(input) {
  return `${input.node_id}.${input.field_name}`;
}

/**
 * Read a swept parameter's values out of what the user typed.
 *
 * Comma-separated, because one field that reads like `4, 6, 8` beats a widget
 * per value. Returns `null` when a token cannot be read as the field's type, so
 * the row can say so instead of queueing something surprising.
 */
export function parseValueList(text, input) {
  const tokens = String(text ?? "")
    .split(",")
    .map((t) => t.trim())
    .filter((t) => t.length > 0);
  if (!tokens.length) return null;
  const kind = controlKind(input);
  if (kind === "combo") {
    const choices = input?.widget?.choices || [];
    // A truncated list cannot vouch for a name it has not been told about.
    if (choices.length && !input?.widget?.truncated) {
      if (tokens.some((t) => !choices.includes(t))) return null;
    }
    return tokens;
  }
  const values = tokens.map((t) => (kind === "int" || kind === "seed" ? Math.trunc(Number(t)) : Number(t)));
  return values.every((v) => Number.isFinite(v)) ? values : null;
}

/** How many tasks a `vary` map queues: the product of its axes. */
export function runCount(vary) {
  return Object.values(vary || {}).reduce((total, axis) => {
    if (Array.isArray(axis)) return total * Math.max(1, axis.length);
    if (typeof axis === "number") return total * Math.max(1, axis);
    if (axis && typeof axis === "object") {
      if (Array.isArray(axis.values) && axis.values.length) return total * axis.values.length;
      return total * Math.max(1, axis.count || 1);
    }
    return total;
  }, 1);
}

/** The most tasks one queue request may expand into — the backend's own cap. */
export const MAX_FANOUT = 64;
