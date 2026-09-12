use std::time::Instant;

use gpui::{AnimationExt as _, AnyElement, ElementId, div, prelude::*, px};

use crate::motion::{CONTENT_DURATION, lerp};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CardGridVisual {
    pub(crate) from_columns: u16,
    pub(crate) to_columns: u16,
    pub(crate) from_width: f32,
    pub(crate) to_width: f32,
    pub(crate) epoch: u64,
    pub(crate) animating: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct CardGridMotion {
    initialized: bool,
    from_columns: u16,
    to_columns: u16,
    from_width: f32,
    to_width: f32,
    epoch: u64,
    started_at: Option<Instant>,
}

impl CardGridMotion {
    pub(crate) fn prepare(
        &mut self,
        columns: u16,
        available_width: f32,
        now: Instant,
        reduced_motion: bool,
    ) -> CardGridVisual {
        let width = if available_width.is_finite() {
            available_width.max(0.)
        } else {
            0.
        };
        if !self.initialized {
            self.initialized = columns > 0;
            self.from_columns = columns;
            self.to_columns = columns;
            self.from_width = width;
            self.to_width = width;
            self.started_at = None;
        } else if self.to_columns != columns {
            self.from_columns = self.to_columns;
            self.from_width = self.to_width;
            self.to_columns = columns;
            self.to_width = width;
            self.epoch = self.epoch.wrapping_add(1);
            self.started_at = (!reduced_motion && self.from_columns > 0).then_some(now);
        } else {
            self.to_width = width;
            if self.started_at.is_none() {
                self.from_width = width;
            }
        }

        if reduced_motion {
            self.from_columns = self.to_columns;
            self.from_width = self.to_width;
            self.started_at = None;
        } else if let Some(started_at) = self.started_at {
            if now.saturating_duration_since(started_at) >= CONTENT_DURATION {
                self.from_columns = self.to_columns;
                self.from_width = self.to_width;
                self.started_at = None;
            }
        }

        CardGridVisual {
            from_columns: self.from_columns,
            to_columns: self.to_columns,
            from_width: self.from_width,
            to_width: self.to_width,
            epoch: self.epoch,
            animating: self.started_at.is_some(),
        }
    }

    pub(crate) fn visual(&self) -> CardGridVisual {
        CardGridVisual {
            from_columns: self.from_columns,
            to_columns: self.to_columns,
            from_width: self.from_width,
            to_width: self.to_width,
            epoch: self.epoch,
            animating: self.started_at.is_some(),
        }
    }

    #[cfg(test)]
    pub(crate) fn is_animating(&self) -> bool {
        self.started_at.is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CardSlot {
    x: f32,
    y: f32,
    width: f32,
}

fn card_slot(
    index: usize,
    columns: u16,
    available_width: f32,
    gap: f32,
    row_pitch: f32,
) -> CardSlot {
    let columns = usize::from(columns.max(1));
    let card_width =
        ((available_width - gap * columns.saturating_sub(1) as f32) / columns as f32).max(0.);
    let col = index % columns;
    let row = index / columns;
    CardSlot {
        x: col as f32 * (card_width + gap),
        y: row as f32 * row_pitch,
        width: card_width,
    }
}

fn row_pitch(available_width: f32, columns: u16, gap: f32, height_extra: f32) -> f32 {
    let columns = usize::from(columns.max(1));
    let card_width =
        ((available_width - gap * columns.saturating_sub(1) as f32) / columns as f32).max(0.);
    card_width + height_extra + gap
}

#[cfg(test)]
fn content_height(
    count: usize,
    columns: u16,
    available_width: f32,
    gap: f32,
    height_extra: f32,
) -> f32 {
    if count == 0 {
        return 0.;
    }
    let rows = count.div_ceil(usize::from(columns.max(1)));
    let pitch = row_pitch(available_width, columns, gap, height_extra);
    (rows as f32 * pitch - gap).max(0.)
}

fn card_cell_delta(
    index: usize,
    visual: CardGridVisual,
    gap: f32,
    height_extra: f32,
) -> (f32, f32, f32, f32) {
    let from_pitch = row_pitch(visual.from_width, visual.from_columns, gap, height_extra);
    let to_pitch = row_pitch(visual.to_width, visual.to_columns, gap, height_extra);
    let from = card_slot(
        index,
        visual.from_columns,
        visual.from_width,
        gap,
        from_pitch,
    );
    let to = card_slot(index, visual.to_columns, visual.to_width, gap, to_pitch);
    (from.x - to.x, from.y - to.y, from.width, to.width)
}

pub(crate) fn animate_grid_card(
    card: AnyElement,
    index: usize,
    visual: CardGridVisual,
    gap: f32,
    height_extra: f32,
) -> AnyElement {
    if !visual.animating {
        return card;
    }
    let (from_left, from_top, from_width, to_width) =
        card_cell_delta(index, visual, gap, height_extra);
    div()
        .relative()
        .child(card)
        .with_animation(
            ElementId::named_usize(format!("grid-card-{index}"), visual.epoch as usize),
            crate::motion::content(),
            move |this, delta| {
                this.left(px(lerp(from_left, 0., delta)))
                    .top(px(lerp(from_top, 0., delta)))
                    .w(px(lerp(from_width, to_width, delta)))
            },
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{CardGridMotion, card_slot, content_height, row_pitch};
    use crate::motion::CONTENT_DURATION;
    use std::time::{Duration, Instant};

    #[test]
    fn first_column_commit_does_not_animate() {
        let mut motion = CardGridMotion::default();
        let now = Instant::now();
        let visual = motion.prepare(6, 900., now, false);
        assert!(!visual.animating);
        assert_eq!(visual.to_columns, 6);
        assert!(!motion.is_animating());
    }

    #[test]
    fn column_change_animates_until_the_content_duration() {
        let mut motion = CardGridMotion::default();
        let now = Instant::now();
        motion.prepare(6, 900., now, false);
        let started = motion.prepare(4, 700., now, false);
        assert!(started.animating);
        assert_eq!(started.from_columns, 6);
        assert_eq!(started.to_columns, 4);
        assert!(motion.is_animating());

        let settled = motion.prepare(4, 700., now + CONTENT_DURATION, false);
        assert!(!settled.animating);
        assert_eq!(settled.from_columns, 4);
        assert!(!motion.is_animating());
    }

    #[test]
    fn reduced_motion_snaps_the_new_columns() {
        let mut motion = CardGridMotion::default();
        let now = Instant::now();
        motion.prepare(6, 900., now, false);
        let visual = motion.prepare(4, 700., now + Duration::from_millis(1), true);
        assert!(!visual.animating);
        assert_eq!(visual.to_columns, 4);
        assert_eq!(visual.from_columns, 4);
    }

    #[test]
    fn slots_fill_rows_left_to_right() {
        let first = card_slot(0, 3, 300., 12., 100.);
        let second = card_slot(1, 3, 300., 12., 100.);
        let wrapped = card_slot(3, 3, 300., 12., 100.);
        assert_eq!(first.x, 0.);
        assert!(second.x > first.x);
        assert_eq!(wrapped.x, 0.);
        assert_eq!(wrapped.y, 100.);
        assert_eq!(row_pitch(300., 3, 12., 52.), first.width + 52. + 12.);
        assert_eq!(
            content_height(4, 3, 300., 12., 52.),
            row_pitch(300., 3, 12., 52.) * 2. - 12.
        );
    }

    #[test]
    fn cell_delta_starts_from_the_previous_slot() {
        let visual = super::CardGridVisual {
            from_columns: 3,
            to_columns: 2,
            from_width: 300.,
            to_width: 300.,
            epoch: 1,
            animating: true,
        };
        let (left, top, from_width, to_width) = super::card_cell_delta(3, visual, 12., 52.);
        let from = card_slot(3, 3, 300., 12., row_pitch(300., 3, 12., 52.));
        let to = card_slot(3, 2, 300., 12., row_pitch(300., 2, 12., 52.));
        assert!((left - (from.x - to.x)).abs() < 0.01);
        assert!((top - (from.y - to.y)).abs() < 0.01);
        assert_eq!(from_width, from.width);
        assert_eq!(to_width, to.width);
        assert!(top < 0.);
    }
}
