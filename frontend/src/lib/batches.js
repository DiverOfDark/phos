/**
 * The confirm sheet's bookkeeping, and none of its rules.
 *
 * The sheet is the most important screen in FR7: it is the last thing between
 * a person and forty-one hours of GPU. Its numbers have to be legible at a
 * glance, which means they are formatted, not printed — 6658 is a number,
 * "6,658" is a count, and "148000 s" is neither.
 */

/** Thousands-separated, the way the sheet reads counts. */
export function formatCount(n) {
  if (n === null || n === undefined || Number.isNaN(Number(n))) return '—'
  return Number(n).toLocaleString('en-US')
}

/**
 * GPU time at the coarsest unit that still says something.
 *
 * A batch is measured in hours or days; showing 147,600 seconds is technically
 * true and practically useless. Under a minute stays in seconds because that is
 * a batch of one and the person should see it is trivial.
 */
export function formatGpu(seconds) {
  const s = Number(seconds)
  if (!Number.isFinite(s) || s <= 0) return '—'
  if (s < 60) return `${Math.round(s)} s`
  if (s < 3600) return `${Math.round(s / 60)} min`
  const hours = s / 3600
  if (hours < 48) return `${hours < 10 ? hours.toFixed(1) : Math.round(hours)} h`
  const days = Math.floor(hours / 24)
  const rest = Math.round(hours - days * 24)
  return rest ? `${days} d ${rest} h` : `${days} d`
}

/** Disk in the unit a person has a feel for. Binary, like every disk tool. */
export function formatBytes(bytes) {
  const b = Number(bytes)
  if (!Number.isFinite(b) || b <= 0) return '—'
  const units = ['B', 'KB', 'MB', 'GB', 'TB', 'PB']
  let value = b
  let i = 0
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024
    i += 1
  }
  const rounded = value >= 100 || i === 0 ? Math.round(value) : value.toFixed(1)
  return `${rounded} ${units[i]}`
}

/** Minutes from midnight as `HH:MM`. */
export function timeFromMinutes(minutes) {
  const m = Number(minutes)
  if (!Number.isFinite(m)) return ''
  const wrapped = ((Math.round(m) % 1440) + 1440) % 1440
  const h = String(Math.floor(wrapped / 60)).padStart(2, '0')
  const mm = String(wrapped % 60).padStart(2, '0')
  return `${h}:${mm}`
}

/** `HH:MM` back to minutes from midnight. `null` for anything else. */
export function minutesFromTime(text) {
  const match = /^(\d{1,2}):(\d{2})$/.exec(String(text || '').trim())
  if (!match) return null
  const h = Number(match[1])
  const m = Number(match[2])
  if (h > 23 || m > 59) return null
  return h * 60 + m
}

/**
 * The window as the sheet says it.
 *
 * An en dash rather than a hyphen, because this is a range and the schedule
 * register reads it as one.
 */
export function formatWindow(startMinute, endMinute) {
  if (startMinute === null || startMinute === undefined) return ''
  if (endMinute === null || endMinute === undefined) return ''
  return `${timeFromMinutes(startMinute)}–${timeFromMinutes(endMinute)}`
}

/**
 * The fan-out down a line, as the multiplier the sheet shows.
 *
 * One stage that sweeps four seeds is `×4`; two that sweep are `×4 · ×2`. A
 * line with no sweep anywhere gets nothing, because `×1` is noise.
 */
export function fanoutSummary(stages) {
  const multipliers = (stages || []).filter((s) => Number(s.fanout) > 1)
  if (!multipliers.length) return ''
  return multipliers.map((s) => `×${s.fanout}`).join(' · ')
}

/**
 * Whether an estimate is worth showing as a promise or as a ceiling.
 *
 * Two things make it a ceiling and they are different: a hold point means a
 * person will cut the work below it by choosing, and a guessed stage means we
 * have never run the thing. The sheet says which.
 */
export function estimateCaveats(preview) {
  if (!preview) return []
  const out = []
  if (preview.has_hold) {
    out.push('This line holds for review, so a person choosing fewer takes will cut it.')
  }
  if (preview.guessed_stages > 0) {
    const s = preview.guessed_stages === 1 ? 'stage has' : 'stages have'
    out.push(`${preview.guessed_stages} ${s} never run here — those numbers are a guess.`)
  }
  return out
}

/**
 * Is this batch feeding, waiting, or over?
 *
 * `paused` is not something a person did; it is a cap saying "not now", and the
 * board colours it as waiting rather than as a fault.
 */
export function batchStatusColor(status) {
  switch (status) {
    case 'completed':
      return 'var(--status-ready)'
    case 'stopped':
      return 'var(--status-stopped)'
    case 'paused':
      return 'var(--status-degraded)'
    default:
      return 'var(--status-building)'
  }
}

/**
 * How far a batch has got, 0–100.
 *
 * Against the number the person agreed to at Send, not against what the query
 * says now — the query is re-asked every tick and the library moves, so
 * measuring against a moving total would make the bar go backwards.
 */
export function batchProgress(batch) {
  const target = Number(batch?.matched_total) - Number(batch?.skipped_total || 0)
  const done = Number(batch?.runs_completed || 0) + Number(batch?.runs_failed || 0)
    + Number(batch?.runs_cancelled || 0)
  if (!Number.isFinite(target) || target <= 0) return 0
  return Math.max(0, Math.min(100, Math.round((done / target) * 100)))
}

/**
 * The selection a gallery filter comes to, in the shape the batch API takes.
 *
 * Drops empty strings rather than sending them: `person_id=""` is not "no
 * person", it is a person whose id is the empty string, and the backend would
 * dutifully match nothing.
 */
export function selectionFromQuery(query) {
  const cleaned = {}
  for (const key of ['q', 'person_id', 'status', 'from', 'to']) {
    const value = query?.[key]
    if (value !== null && value !== undefined && String(value).trim() !== '') {
      cleaned[key] = String(value).trim()
    }
  }
  return { kind: 'query', query: cleaned }
}

/** A short human name for what a selection points at. Mirrors the backend's. */
export function selectionShorthand(selection) {
  if (!selection) return ''
  if (selection.kind === 'ids') return `${formatCount(selection.ids?.length || 0)} selected`
  const q = selection.query || {}
  const parts = []
  if (q.person_id) parts.push('person')
  const from = q.from ? String(q.from).slice(0, 4) : ''
  const to = q.to ? String(q.to).slice(0, 4) : ''
  if (from && to) parts.push(`${from}–${to}`)
  else if (from) parts.push(`${from}–`)
  else if (to) parts.push(`–${to}`)
  if (q.status) parts.push(q.status)
  if (q.q) parts.push(`“${q.q}”`)
  return parts.length ? parts.join(' · ') : 'whole library'
}

/** Caps, ready to POST. Anything unset is left out rather than sent as null. */
export function capsPayload(form) {
  const caps = {}
  if (form.dailyTaskCap) caps.daily_task_cap = Number(form.dailyTaskCap)
  if (form.windowEnabled) {
    const start = minutesFromTime(form.windowStart)
    const end = minutesFromTime(form.windowEnd)
    // Half a window is no window — the backend reads it that way too, and
    // sending one half would look like it had been accepted.
    if (start !== null && end !== null) {
      caps.window_start_minute = start
      caps.window_end_minute = end
    }
  }
  if (form.diskFloorGb) caps.disk_floor_bytes = Math.round(Number(form.diskFloorGb) * 1024 ** 3)
  if (form.maxOutstandingHolds) caps.max_outstanding_holds = Number(form.maxOutstandingHolds)
  return caps
}
