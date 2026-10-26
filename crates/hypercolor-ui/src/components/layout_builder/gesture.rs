//! How a canvas gesture lands in the editor.
//!
//! A drag or resize paints the DOM directly and holds its own copy of the
//! outputs from the press. When it ends, only the placement fields it
//! changed move into the editor as the editor stands then, so a change
//! that reached the editor from elsewhere mid-gesture is never put back.
//! The editor's next commit turns those fields into ledger intents.

use hypercolor_types::spatial::Output;

/// Move one field from `from` to `to` when it changed. A `guarded` move
/// also leaves a field that `target` holds at a value of its own.
fn carry<T: PartialEq + Copy>(target: &mut T, from: T, to: T, guarded: bool) {
    if to != from && (!guarded || *target == from) {
        *target = to;
    }
}

/// Carry the placement fields that changed from `baseline` to `edited`
/// onto `target`. A `guarded` carry skips a field `target` itself changed
/// from `baseline`.
fn carry_placement(target: &mut Output, baseline: &Output, edited: &Output, guarded: bool) {
    carry(
        &mut target.position.x,
        baseline.position.x,
        edited.position.x,
        guarded,
    );
    carry(
        &mut target.position.y,
        baseline.position.y,
        edited.position.y,
        guarded,
    );
    carry(&mut target.size.x, baseline.size.x, edited.size.x, guarded);
    carry(&mut target.size.y, baseline.size.y, edited.size.y, guarded);
    carry(
        &mut target.rotation,
        baseline.rotation,
        edited.rotation,
        guarded,
    );
    carry(&mut target.scale, baseline.scale, edited.scale, guarded);
    carry(
        &mut target.orientation,
        baseline.orientation,
        edited.orientation,
        guarded,
    );
}

/// Carry what changed from `before` to `after` onto `outputs`, matched by
/// id, for a snapshot with edits of its own: a field the snapshot already
/// changed from `before` keeps its value, and only what it still holds as
/// `before` had it moves to `after`.
pub fn rebase_unedited(outputs: &mut [Output], before: &[Output], after: &[Output]) {
    for output in outputs {
        if let Some(from) = before.iter().find(|from| from.id == output.id)
            && let Some(to) = after.iter().find(|to| to.id == output.id)
        {
            carry_placement(output, from, to, true);
        }
    }
}

/// The editor's outputs once a canvas gesture ends: `current` as the
/// editor holds them now, each carrying only the placement fields the
/// gesture changed from `base`, the outputs at its press. A change that
/// reached the editor from elsewhere while the gesture ran stays, so a
/// release never puts back another client's move.
#[must_use]
pub fn merge_gesture(current: &[Output], base: &[Output], gestured: &[Output]) -> Vec<Output> {
    current
        .iter()
        .map(|output| {
            let mut merged = output.clone();
            if let Some(before) = base.iter().find(|before| before.id == output.id)
                && let Some(after) = gestured.iter().find(|after| after.id == output.id)
            {
                carry_placement(&mut merged, before, after, false);
            }
            merged
        })
        .collect()
}
