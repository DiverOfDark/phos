import { clsx } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs) {
  return twMerge(clsx(inputs));
}

/**
 * Is this detected workflow input one the console can offer as a text box?
 *
 * Since the backend reads ComfyUI's `/object_info`, a workflow's inputs also
 * carry seeds, step counts, cfg scales and model pickers, each with a `widget`
 * saying what it is. Those want controls of their own; until there are some,
 * only text is editable — which is exactly what was editable before.
 * An input with no `widget` came from the fallback heuristics and is a string
 * by construction.
 */
export function isTextInput(input) {
  if (!input || input.node_type === "LoadImage") return false;
  if (!input.widget) return typeof input.current_value === "string";
  return input.widget.kind === "text";
}
