use std::time::Instant;

use crate::artwork_cache::global_cache;
use gpui::{
    AnyElement, App, Element, ElementId, GlobalElementId, ImageSource, InspectorElementId,
    IntoElement, LayoutId, Pixels, Resource, StyleRefinement, Styled, StyledImage, Window, img,
};

/// How a reveal slot should be drawn for a given readiness sample. This is the
/// single decision every remote image in the app shares: a placeholder until
/// the artwork is ready, then a short fade, then a settled image.
///
/// The decision logic is pure: `advance` maps what the slot showed last frame
/// plus a readiness sample to the next phase, so it can be tested without a
/// window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArtworkRevealPhase {
    /// The image is not usable yet. Render nothing; the site's placeholder
    /// frame shows through.
    Waiting,
    /// The image became usable after this slot was already showing. The image
    /// renders while its opacity animates from 0 to 1.
    Fading,
    /// The image is shown with no transition. Fresh mounts of already-cached
    /// artwork settle immediately so scrolling a warm cache never replays the
    /// fade, and reduced motion never animates.
    Settled,
}

/// Snapshot of the inputs `advance` needs: what the slot showed last frame,
/// whether the requested source differs from it, and whether that source is
/// usable right now.
#[derive(Clone, Debug)]
pub(crate) struct ArtworkRevealInput {
    /// The source and phase the slot showed on the previous frame, if any.
    /// `None` means the slot is mounted for the first time.
    pub(crate) previous: Option<(Resource, ArtworkRevealPhase)>,
    /// Identity of the source the slot is being asked to show now.
    pub(crate) source: Resource,
    /// Whether that source is already usable (finished loading in the artwork
    /// cache, with or without an error).
    pub(crate) ready: bool,
}

impl ArtworkRevealPhase {
    /// Decide the next phase for a reveal slot.
    ///
    /// - not ready: wait, whatever came before (a slow download, a source
    ///   change while fading, or an eviction all fall back to the
    ///   placeholder);
    /// - ready on a fresh mount, on a source change, under reduced motion, or
    ///   once this source is already showing: settle instantly;
    /// - ready after this slot was visibly waiting for this same source: fade
    ///   in over whatever the site keeps mounted beneath.
    pub(crate) fn advance(input: &ArtworkRevealInput, reduce_motion: bool) -> Self {
        if !input.ready {
            return Self::Waiting;
        }

        let same_source_as_last_frame = input
            .previous
            .as_ref()
            .is_some_and(|(source, _)| *source == input.source);
        if !same_source_as_last_frame || reduce_motion {
            return Self::Settled;
        }

        match input.previous.as_ref().map(|(_, phase)| *phase) {
            // A fade in progress keeps running; only a finished fade settles.
            Some(Self::Waiting) | Some(Self::Fading) => Self::Fading,
            Some(Self::Settled) | None => Self::Settled,
        }
    }

    /// Settle a fade that has run its course. Callers feed the result back
    /// through `advance` on the next frame, which keeps the slot settled
    /// without scheduling further animation frames.
    pub(crate) fn settled_after_fade(self, fade_complete: bool) -> Self {
        match self {
            Self::Fading if fade_complete => Self::Settled,
            phase => phase,
        }
    }

    /// The opacity the image should paint with at `delta` progress through the
    /// fade. Waiting paints nothing, settled paints fully, fading interpolates
    /// with the shared ease-out curve.
    pub(crate) fn opacity_at(self, delta: f32) -> f32 {
        match self {
            Self::Waiting => 0.0,
            Self::Fading => crate::motion::lerp(0.0, 1.0, ease_out_quint_01(delta)),
            Self::Settled => 1.0,
        }
    }
}

/// Ease-out quint on a unit input, matching the app's other content motion.
/// Exposed separately because gpui's easing functions are not unit-clamped.
pub(crate) fn ease_out_quint_01(delta: f32) -> f32 {
    let clamped = crate::motion::clamp_unit(delta);
    1. - (1. - clamped).powi(5)
}

#[derive(Clone, Debug)]
struct ArtworkRevealState {
    source: Option<Resource>,
    phase: ArtworkRevealPhase,
    fade_started_at: Option<Instant>,
}

impl Default for ArtworkRevealState {
    fn default() -> Self {
        Self {
            source: None,
            phase: ArtworkRevealPhase::Waiting,
            fade_started_at: None,
        }
    }
}

impl ArtworkRevealState {
    fn fade_delta(&self) -> f32 {
        let Some(started_at) = self.fade_started_at else {
            return 1.0;
        };
        started_at.elapsed().as_secs_f32() / crate::motion::ARTWORK_REVEAL_DURATION.as_secs_f32()
    }
}

/// An image that appears through the app's shared artwork reveal behavior.
pub(crate) struct ArtworkReveal {
    id: ElementId,
    source: ImageSource,
    resource: Option<Resource>,
    style: StyleRefinement,
    // ObjectFit lacks Clone in this gpui revision, so the element stores a
    // variant constructor and builds a fresh value per frame.
    object_fit: fn() -> gpui::ObjectFit,
}

/// Build the shared reveal element for a remote or on-disk artwork source.
///
/// `id` must be stable for the artwork slot across renders (a card id, a row
/// key) because the reveal keeps its fade progress in element state keyed by
/// the element id path. The image is fitted with `ObjectFit::Cover`, matching
/// every artwork surface in the app; use [`ArtworkReveal::object_fit`] to
/// override.
pub(crate) fn artwork_reveal(
    id: impl Into<ElementId>,
    source: impl Into<ImageSource>,
) -> ArtworkReveal {
    let source = source.into();
    let resource = match &source {
        ImageSource::Resource(resource) => Some(resource.clone()),
        // In-memory and custom sources have nothing to wait for; they render
        // settled from the first frame.
        ImageSource::Render(_) | ImageSource::Image(_) | ImageSource::Custom(_) => None,
    };
    ArtworkReveal {
        id: id.into(),
        source,
        resource,
        style: StyleRefinement::default(),
        object_fit: || gpui::ObjectFit::Cover,
    }
}

impl ArtworkReveal {
    /// Set how the revealed image fits its bounds.
    pub(crate) fn object_fit(mut self, object_fit: gpui::ObjectFit) -> Self {
        self.object_fit = match object_fit {
            gpui::ObjectFit::Fill => || gpui::ObjectFit::Fill,
            gpui::ObjectFit::Contain => || gpui::ObjectFit::Contain,
            gpui::ObjectFit::ScaleDown => || gpui::ObjectFit::ScaleDown,
            gpui::ObjectFit::None => || gpui::ObjectFit::None,
            gpui::ObjectFit::Cover => || gpui::ObjectFit::Cover,
        };
        self
    }

    fn sample_ready(&self, cx: &App) -> bool {
        let Some(resource) = &self.resource else {
            return true;
        };
        let Some(cache) = global_cache(cx) else {
            // No shell cache is registered (tests, previews): nothing can be
            // polled, so render settled rather than waiting forever.
            return true;
        };
        cache.read(cx).is_loaded(resource)
    }

    fn advance(&self, state: &mut ArtworkRevealState, window: &mut Window, cx: &App) {
        let fade_complete = state.fade_started_at.is_some_and(|started_at| {
            started_at.elapsed() >= crate::motion::ARTWORK_REVEAL_DURATION
        });
        state.phase = state.phase.settled_after_fade(fade_complete);

        let Some(resource) = &self.resource else {
            state.phase = ArtworkRevealPhase::Settled;
            state.fade_started_at = None;
            return;
        };

        let input = ArtworkRevealInput {
            previous: state.source.clone().map(|source| (source, state.phase)),
            source: resource.clone(),
            ready: self.sample_ready(cx),
        };
        let phase = ArtworkRevealPhase::advance(&input, cx.reduce_motion());
        state.fade_started_at = match phase {
            ArtworkRevealPhase::Fading if state.phase == ArtworkRevealPhase::Fading => {
                state.fade_started_at
            }
            ArtworkRevealPhase::Fading => Some(Instant::now()),
            _ => None,
        };
        state.phase = phase;
        state.source = Some(resource.clone());
        if phase == ArtworkRevealPhase::Fading {
            window.request_animation_frame();
        }
    }
}

impl Element for ArtworkReveal {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        window.with_element_state(
            global_id.expect("ArtworkReveal must have an element id"),
            |state, window| {
                let mut state = state.unwrap_or_default();
                self.advance(&mut state, window, cx);

                let mut image = img(self.source.clone()).object_fit((self.object_fit)());
                *image.style() = self.style.clone();
                let mut image = image
                    .opacity(state.phase.opacity_at(state.fade_delta()))
                    .absolute()
                    .inset_0()
                    .into_any_element();
                let layout_id = image.request_layout(window, cx);
                ((layout_id, image), state)
            },
        )
    }

    fn prepaint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: gpui::Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: gpui::Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.paint(window, cx);
    }
}

impl IntoElement for ArtworkReveal {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Styled for ArtworkReveal {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(value: &str) -> Resource {
        Resource::Uri(value.into())
    }

    #[test]
    fn fresh_mount_waits_then_fades_when_ready() {
        let source = uri("https://example.test/cover.jpg");

        // First frame: nothing loaded yet, so the placeholder shows.
        let input = ArtworkRevealInput {
            previous: None,
            source: source.clone(),
            ready: false,
        };
        assert_eq!(
            ArtworkRevealPhase::advance(&input, false),
            ArtworkRevealPhase::Waiting
        );

        // The same slot, same source, still not ready: keep waiting.
        let input = ArtworkRevealInput {
            previous: Some((source.clone(), ArtworkRevealPhase::Waiting)),
            source: source.clone(),
            ready: false,
        };
        assert_eq!(
            ArtworkRevealPhase::advance(&input, false),
            ArtworkRevealPhase::Waiting
        );

        // The load finishes: the slot was visibly waiting, so it fades in.
        let input = ArtworkRevealInput {
            previous: Some((source.clone(), ArtworkRevealPhase::Waiting)),
            source: source.clone(),
            ready: true,
        };
        assert_eq!(
            ArtworkRevealPhase::advance(&input, false),
            ArtworkRevealPhase::Fading
        );

        // While the fade runs it keeps fading rather than restarting.
        let input = ArtworkRevealInput {
            previous: Some((source.clone(), ArtworkRevealPhase::Fading)),
            source: source.clone(),
            ready: true,
        };
        assert_eq!(
            ArtworkRevealPhase::advance(&input, false),
            ArtworkRevealPhase::Fading
        );

        // Once the fade completes it settles and stays settled.
        assert_eq!(
            ArtworkRevealPhase::Fading.settled_after_fade(true),
            ArtworkRevealPhase::Settled
        );
        let input = ArtworkRevealInput {
            previous: Some((source, ArtworkRevealPhase::Settled)),
            source: uri("https://example.test/cover.jpg"),
            ready: true,
        };
        assert_eq!(
            ArtworkRevealPhase::advance(&input, false),
            ArtworkRevealPhase::Settled
        );
    }

    #[test]
    fn already_cached_artwork_mounts_settled() {
        // A card scrolling into view from a warm cache must not replay the
        // fade; otherwise every scroll replays a wave of fades.
        let input = ArtworkRevealInput {
            previous: None,
            source: uri("https://example.test/cover.jpg"),
            ready: true,
        };
        assert_eq!(
            ArtworkRevealPhase::advance(&input, false),
            ArtworkRevealPhase::Settled
        );
    }

    #[test]
    fn reduced_motion_never_fades() {
        let source = uri("https://example.test/cover.jpg");
        let input = ArtworkRevealInput {
            previous: Some((source.clone(), ArtworkRevealPhase::Waiting)),
            source: source.clone(),
            ready: true,
        };
        assert_eq!(
            ArtworkRevealPhase::advance(&input, true),
            ArtworkRevealPhase::Settled
        );
    }

    #[test]
    fn source_change_on_a_live_slot_settles_without_a_fade_wave() {
        // List recycling (queue reorder, virtualized rows) reuses a slot with
        // a different image. Settling instantly keeps a reorder from firing a
        // wave of fades; navigation pop-in is covered by the fresh-mount
        // waiting path instead.
        let first = uri("https://example.test/first.jpg");
        let second = uri("https://example.test/second.jpg");
        let input = ArtworkRevealInput {
            previous: Some((first, ArtworkRevealPhase::Settled)),
            source: second.clone(),
            ready: true,
        };
        assert_eq!(
            ArtworkRevealPhase::advance(&input, false),
            ArtworkRevealPhase::Settled
        );

        // A source change to a not-ready image falls back to the placeholder
        // and then fades in once the new image is ready.
        let third = uri("https://example.test/third.jpg");
        let input = ArtworkRevealInput {
            previous: Some((second, ArtworkRevealPhase::Settled)),
            source: third.clone(),
            ready: false,
        };
        assert_eq!(
            ArtworkRevealPhase::advance(&input, false),
            ArtworkRevealPhase::Waiting
        );
        let input = ArtworkRevealInput {
            previous: Some((third, ArtworkRevealPhase::Waiting)),
            source: uri("https://example.test/third.jpg"),
            ready: true,
        };
        assert_eq!(
            ArtworkRevealPhase::advance(&input, false),
            ArtworkRevealPhase::Fading
        );
    }

    #[test]
    fn an_unfinished_fade_keeps_fading() {
        let source = uri("https://example.test/cover.jpg");
        assert_eq!(
            ArtworkRevealPhase::Fading.settled_after_fade(false),
            ArtworkRevealPhase::Fading
        );
        assert_eq!(
            ArtworkRevealPhase::Waiting.settled_after_fade(true),
            ArtworkRevealPhase::Waiting
        );
        assert_eq!(
            ArtworkRevealPhase::Settled.settled_after_fade(true),
            ArtworkRevealPhase::Settled
        );
        let _ = source;
    }

    #[test]
    fn opacity_follows_the_phase_and_eases_out() {
        assert_eq!(ArtworkRevealPhase::Waiting.opacity_at(0.5), 0.0);
        assert_eq!(ArtworkRevealPhase::Settled.opacity_at(0.0), 1.0);

        let start = ArtworkRevealPhase::Fading.opacity_at(0.0);
        let quarter = ArtworkRevealPhase::Fading.opacity_at(0.25);
        let half = ArtworkRevealPhase::Fading.opacity_at(0.5);
        let end = ArtworkRevealPhase::Fading.opacity_at(1.0);
        assert_eq!(start, 0.0);
        assert_eq!(end, 1.0);
        // Ease-out quint starts fast: the first quarter covers more ground
        // than the second quarter does.
        assert!(quarter > 0.5);
        assert!(quarter - start > end - half);
    }

    #[test]
    fn fade_duration_matches_the_motion_system() {
        assert_eq!(
            crate::motion::ARTWORK_REVEAL_DURATION,
            std::time::Duration::from_millis(180)
        );
    }

    #[test]
    fn ease_out_quint_is_unit_clamped() {
        assert_eq!(ease_out_quint_01(-1.0), 0.0);
        assert_eq!(ease_out_quint_01(0.0), 0.0);
        assert_eq!(ease_out_quint_01(1.0), 1.0);
        assert_eq!(ease_out_quint_01(2.0), 1.0);
    }
}
