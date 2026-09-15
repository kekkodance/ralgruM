use super::*;
use gpui::ImageSource;
use url::Url;

#[derive(Clone, Copy)]
pub(crate) enum CardKind {
    Album,
    Artist,
    Flow,
    Playlist,
    Other,
}

pub(super) fn card_artwork(
    artwork: CardArtworkSource,
    kind: CardKind,
    provider: Option<Provider>,
    badge: Option<&str>,
    id: &str,
) -> AnyElement {
    let icon = match kind {
        CardKind::Artist => LocalIcon::User,
        CardKind::Playlist => LocalIcon::ListUl,
        _ => LocalIcon::CompactDisc,
    };
    let image_source = match &artwork {
        CardArtworkSource::Remote(artwork) => (!artwork.is_empty()).then(|| artwork.clone().into()),
        CardArtworkSource::Local(path) => path.clone().map(ImageSource::from),
    };
    div()
        .relative()
        .w_full()
        .aspect_square()
        .overflow_hidden()
        .rounded(px(8.))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(SURFACE_RAISED))
        // The kind icon remains visible while remote artwork loads. The
        // reveal element fades the downloaded image in over this placeholder.
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(local_icon(icon, 0x52525b).size(px(30.))),
        )
        .when_some(image_source, |this, image_source| {
            this.child(
                crate::artwork_reveal::artwork_reveal(
                    format!("collection-artwork-{id}"),
                    image_source,
                )
                .size_full()
                .rounded(px(8.)),
            )
        })
        .when_some(provider, |this, provider| {
            this.child(
                div()
                    .absolute()
                    .top(px(7.))
                    .left(px(7.))
                    .w(px(21.))
                    .h(px(21.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgba(0xffffff1f))
                    .bg(rgba(0x09090bc7))
                    .child(provider_logo(provider).size(px(9.))),
            )
        })
        .when_some(badge.filter(|badge| !badge.is_empty()), |this, badge| {
            this.child(
                div()
                    .absolute()
                    .bottom(px(7.))
                    .right(px(7.))
                    .h(px(19.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(rgba(0xffffff24))
                    .bg(rgba(0x09090bd1))
                    .text_size(px(9.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0xe4e4e7))
                    .when(matches!(kind, CardKind::Playlist), |this| {
                        this.child(
                            div()
                                .relative()
                                .top(px(1.))
                                .child(local_icon(LocalIcon::Music, 0xe4e4e7).size(px(8.))),
                        )
                    })
                    .child(badge.to_uppercase()),
            )
        })
        .into_any_element()
}

pub(super) fn track_artwork_url(artwork: &str) -> String {
    let Ok(mut url) = Url::parse(artwork) else {
        return artwork.to_owned();
    };
    if url.scheme() != "https" {
        return artwork.to_owned();
    }
    let Some(host) = url.host_str() else {
        return artwork.to_owned();
    };
    let path = url.path();
    let Some((directory, filename)) = path.rsplit_once('/') else {
        return artwork.to_owned();
    };
    let resized = if host.eq_ignore_ascii_case("e-cdns-images.dzcdn.net") {
        filename
            .strip_prefix("500x500")
            .filter(|suffix| suffix.starts_with('.') || suffix.starts_with('-'))
            .map(|suffix| format!("120x120{suffix}"))
    } else if host.eq_ignore_ascii_case("sndcdn.com")
        || host.to_ascii_lowercase().ends_with(".sndcdn.com")
    {
        soundcloud_thumbnail_filename(filename)
    } else {
        None
    };
    let Some(resized) = resized else {
        return artwork.to_owned();
    };
    url.set_path(&format!("{directory}/{resized}"));
    url.to_string()
}

fn soundcloud_thumbnail_filename(filename: &str) -> Option<String> {
    if let Some((prefix, suffix)) = filename.rsplit_once("-t500x500")
        && !prefix.is_empty()
        && suffix.starts_with('.')
    {
        return Some(format!("{prefix}-t120x120{suffix}"));
    }
    if let Some((prefix, suffix)) = filename.rsplit_once("-large")
        && !prefix.is_empty()
        && suffix.starts_with('.')
    {
        return Some(format!("{prefix}-t120x120{suffix}"));
    }
    None
}

pub(super) fn artwork_view(artwork: &str, reveal_key: &str) -> AnyElement {
    let artwork = track_artwork_url(artwork);
    div()
        .w(px(TRACK_ARTWORK_SIZE_PX))
        .h(px(TRACK_ARTWORK_SIZE_PX))
        .flex_none()
        .relative()
        .overflow_hidden()
        .rounded(px(6.))
        .bg(rgb(SURFACE_RAISED))
        .when(!artwork.is_empty(), |this| {
            this.child(
                crate::artwork_reveal::artwork_reveal(
                    format!("track-artwork-reveal-{reveal_key}"),
                    artwork,
                )
                .size_full()
                .rounded(px(6.)),
            )
        })
        .into_any_element()
}

pub(super) fn provider_badge(
    provider: Provider,
    responsive: crate::motion::ResponsiveModeVisual,
    motion_key: String,
) -> AnyElement {
    let color = match provider {
        Provider::Deezer => DEEZER,
        Provider::SoundCloud => SOUNDCLOUD,
    };
    let (from_label_width, target_label_width) =
        responsive.endpoints(track_provider_label_width(provider), 0.);
    let (from_opacity, target_opacity) = responsive.endpoints(1., 0.);
    let label = div()
        .flex_none()
        .overflow_hidden()
        .whitespace_nowrap()
        .w(px(target_label_width))
        .opacity(target_opacity)
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(color))
        .child(provider.label())
        .with_animation(
            ElementId::named_usize(
                format!("track-provider-label-{motion_key}"),
                responsive.epoch as usize,
            ),
            crate::motion::content(),
            move |this, delta| {
                this.w(px(crate::motion::lerp(
                    from_label_width,
                    target_label_width,
                    delta,
                )))
                .opacity(crate::motion::lerp(
                    from_opacity,
                    target_opacity,
                    delta,
                ))
            },
        )
        .into_any_element();
    div()
        .h(px(20.))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(4.))
        .child(
            div()
                .relative()
                .top(px(TRACK_PROVIDER_ICON_OPTICAL_OFFSET_PX))
                .id(format!("track-provider-logo-{motion_key}"))
                .app_tooltip(provider.label())
                .child(provider_logo(provider).size(px(11.))),
        )
        .child(label)
        .into_any_element()
}

fn provider_logo(provider: Provider) -> gpui::Svg {
    let (icon, color) = match provider {
        Provider::Deezer => (LocalIcon::Deezer, DEEZER),
        Provider::SoundCloud => (LocalIcon::SoundCloud, SOUNDCLOUD),
    };
    local_icon(icon, color)
}
