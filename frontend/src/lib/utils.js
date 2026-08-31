import { clsx } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs) {
  return twMerge(clsx(inputs));
}

/**
 * Is this detected workflow input one the console can offer a control for?
 *
 * Since the backend reads ComfyUI's `/object_info`, a workflow's inputs also
 * carry seeds, step counts, cfg scales and model pickers, each with a `widget`
 * saying what it is. Numbers and booleans want controls of their own; until
 * there are some, what is editable is what a string override can reach:
 *
 * - `text` — a text box, as before;
 * - a `combo` whose value is a string — a picker over the choices ComfyUI
 *   listed. Before the catalogue existed these were surfaced as plain text
 *   on any class named like a string node, so dropping them would take an
 *   override away from a workflow that had one;
 * - an input with no `widget` — found by the fallback heuristics, and a
 *   string by construction.
 *
 * An override is applied by rewriting a string in the graph, so a combo
 * holding a number or a boolean is not offered: it would be a control that
 * silently did nothing.
 */
export function isEditableInput(input) {
  if (!input || input.node_type === "LoadImage") return false;
  if (!input.widget) return typeof input.current_value === "string";
  if (input.widget.kind === "text") return true;
  return input.widget.kind === "combo" && typeof input.current_value === "string";
}

/** Does this editable input want a picker rather than a text box? */
export function isComboInput(input) {
  return isEditableInput(input) && input.widget?.kind === "combo";
}

/**
 * What the picker for a combo offers, as strings. The value the workflow or
 * preset currently holds is always among them — a snapshot lists at most a
 * few hundred choices (`truncated`), and a model can be uninstalled after
 * the workflow was imported; a picker that cannot show its own value would
 * silently swap it for the first entry.
 */
export function comboChoices(input, current) {
  const choices = (input?.widget?.choices || []).map((c) => String(c));
  if (typeof current === "string" && current !== "" && !choices.includes(current)) {
    return [current, ...choices];
  }
  return choices;
}
