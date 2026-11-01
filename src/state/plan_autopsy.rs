//! Plan autopsy: the reason behind a day's fulfilment rate.
//!
//! [`DayTally::fulfillment`](crate::state::outcome::DayTally::fulfillment)
//! returns a number — "62% fulfilled". It stops there: the user sees a rate
//! but not *which* planned blocks became what. An autopsy is the missing link:
//! for each planned block in a span, the full pair of what was planned and what
//! actually happened — the session it produced (if any), the verdict it earned
//! (if it was judged), and the obstruction that blocked it (if one was
//! recorded).
//!
//! In this codebase "plan" is a rota entry. A named [`Plan`] is a *template*;
//! applying it writes [`RotaEntry`] rows for a day, and those rows are the
//! concrete planned blocks. So the autopsy runs over rota entries for the span,
//! not over the templates — a plan that was never applied produced no block and
//! so has nothing to autopsy.
//!
//! Everything here is pure: slices and explicit parameters in, owned data out,
//! no I/O, no clock reads beyond what is passed in.

use serde::{Deserialize, Serialize};

/// What became of one planned block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutopsyOutcome {
    /// A session landed: the block was carried out.
    Landed,
    /// The block was started but no session landed — abandoned part-way.
    Abandoned,
    /// Never started, and the day it was planned for is over.
    Unreached,
}

/// One planned block, joined to what became of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutopsyRow {
    /// The rota entry's id — the concrete planned block.
    pub rota_entry_id: u64,
    /// The day the block was planned for, `YYYY-MM-DD`.
    pub day: String,
    /// What the block was called.
    pub label: String,
    /// What became of it.
    pub outcome: AutopsyOutcome,
    /// The session the block produced, when it landed, its Unix seconds.
    pub session_at: Option<i64>,
    /// The verdict the session earned, when it was judged.
    pub verdict: Option<crate::journal::VerdictKind>,
    /// The obstruction that blocked the session, when one was recorded.
    pub obstruction: Option<String>,
    /// Whether the block was scheduled (not abandoned, reached).
    pub scheduled: bool,
}

/// Build the autopsy rows for the rota entries whose `day` is in [`from`,
/// `to`] (inclusive, `YYYY-MM-DD`). Rows come back in the order the entries
/// were planned.
pub fn autopsy(
    rota: &[crate::state::rota::RotaEntry],
    verdicts: &[crate::journal::Verdict],
    obstructions: &[crate::journal::obstruction::Obstruction],
    from: &str,
    to: &str,
    today: &str,
) -> Vec<AutopsyRow> {
    let mut rows = Vec::new();
    for e in rota.iter().filter(|e| e.is_live()) {
        if e.day.as_str() < from || e.day.as_str() > to {
            continue;
        }
        // Effective outcome: Pending on a past day reads as Missed via the
        // outcome module's rule; we map Missed -> Unreached.
        let eff = crate::state::outcome::effective(e, today);
        let (outcome, scheduled) = match eff {
            crate::state::outcome::Outcome::Done => (AutopsyOutcome::Landed, true),
            crate::state::outcome::Outcome::Abandoned => (AutopsyOutcome::Abandoned, true),
            crate::state::outcome::Outcome::Missed | crate::state::outcome::Outcome::Pending => {
                (AutopsyOutcome::Unreached, false)
            }
        };
        let session_at = if e.session_at > 0 {
            Some(e.session_at)
        } else {
            None
        };
        let verdict =
            session_at.and_then(|at| crate::journal::verdict::find(verdicts, at).map(|v| v.kind));
        let obstruction = session_at.and_then(|at| {
            crate::journal::obstruction::obstruction_for(obstructions, at).map(|o| o.text.clone())
        });
        rows.push(AutopsyRow {
            rota_entry_id: e.id,
            day: e.day.clone(),
            label: e.label.clone(),
            outcome,
            session_at,
            verdict,
            obstruction,
            scheduled,
        });
    }
    rows
}

/// (landed, abandoned, unreached) counters for the rows.
pub fn by_outcome(rows: &[AutopsyRow]) -> (u32, u32, u32) {
    let mut landed = 0u32;
    let mut abandoned = 0u32;
    let mut unreached = 0u32;
    for r in rows {
        match r.outcome {
            AutopsyOutcome::Landed => landed += 1,
            AutopsyOutcome::Abandoned => abandoned += 1,
            AutopsyOutcome::Unreached => unreached += 1,
        }
    }
    (landed, abandoned, unreached)
}

/// A one-line label for the autopsy, in the requested locale.
pub fn label(rows: &[AutopsyRow], locale_fa: bool) -> String {
    let (landed, abandoned, unreached) = by_outcome(rows);
    if locale_fa {
        format!(
            "{} فرود · {} رها · {} نرسیده",
            crate::calendar::jalali::fa_num(landed as i64),
            crate::calendar::jalali::fa_num(abandoned as i64),
            crate::calendar::jalali::fa_num(unreached as i64)
        )
    } else {
        format!("{landed} landed · {abandoned} abandoned · {unreached} unreached")
    }
}
