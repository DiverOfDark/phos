//! A whole library's worth of shots sent to one line in one action.
//!
//! > *Everything of Grandma before 1990, restore and upscale, run overnight. In
//! > the morning I go through what came out.*
//!
//! # Send a query, not a list
//!
//! Fifty thousand ids in a POST body is a non-starter, and a person does not
//! think in ids anyway. A [`selection::Selection`] is **either** an explicit id
//! list **or** a query in exactly the shape `/api/shots` already takes — person,
//! date range, review status, search text. There is one filter language in Phos
//! and this reuses it rather than growing a second: the query arm holds an
//! [`crate::api::shots::ShotsQuery`] and the SQL comes from
//! `api::shots::shot_conditions`.
//!
//! # Materialise lazily
//!
//! One `batches` row holds the query plus a cursor. Each tick, [`feed`] pulls
//! the next handful of shots and opens runs for them. Fifty thousand run rows
//! are never inserted at once, and three things follow:
//!
//! * **STOP is instant** — there is nothing to unwind.
//! * **The board stays fast** — the queue is what is running, not what might.
//! * **Newly imported matches are picked up for free**, as long as they sort
//!   after the cursor.
//!
//! The cursor is a keyset over `(COALESCE(shots.timestamp,''), shots.id)`
//! ascending, and it is a pair rather than a timestamp because `shots.id` is
//! what makes the order total. An OFFSET would drift the moment an import
//! lands mid-batch.
//!
//! # Guardrails, and why they are v1
//!
//! At this scale the failure mode is not a crash. It is generating more than
//! anyone will ever look at. So before anything is queued there is a confirm
//! sheet — matched, already-done, to-run, tasks, GPU hours, disk — and a batch
//! carries caps: tasks a day, an optional window, a disk floor, and a cap on
//! **outstanding holds**.
//!
//! That last one is the one worth explaining. FR5c lets a stage park its run
//! and ask a person which takes go on; held runs park rather than block, so
//! work continues past them. At batch scale that is a mountain waiting to
//! happen: 3,329 shots through `×4 extend → hold → upscale` produce 13,316
//! clips waiting on a human before any upscale runs. When more of a batch's
//! runs are held than its cap allows, feeding pauses until verdicts bring the
//! number down.
//!
//! The cap counts `runs.status = 'held'` and **never** joins to
//! `held_at_stage`. FR5c's author flagged those two as the one pair of markers
//! in that feature that could in principle disagree; a cap that quietly ignored
//! a held run with a NULL stage would let the mountain grow past the limit it
//! exists to enforce. Where a run is parked is not the cap's business.
//!
//! # Nothing runs on a timer
//!
//! There is no cron here, no scheduled job, no standing order. A batch exists
//! because a person pressed Send. A window only *paces* work already queued,
//! and a saved selection ([`store::save_selection`]) makes a repeat one click —
//! it never fires on its own.
//!
//! # The shape of it
//!
//! * [`plan`] — pure. The cursor, the caps, the estimate. No database.
//! * [`selection`] — what was pointed at, and the statement that resolves it.
//! * [`store`] — batches on disk, and the counts the caps are decided from.
//! * [`feed`] — the tick, and STOP.

pub mod feed;
pub mod plan;
pub mod selection;
pub mod store;

#[cfg(test)]
mod tests;

pub use feed::{feed_batches, stop};
