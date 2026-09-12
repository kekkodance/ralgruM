use gpui::{
    AnyElement, Context, IntoElement, MouseButton, Role, deferred, div, prelude::*, px, rgb, rgba,
};

use crate::{
    app_button::plain_x_button,
    assets::{LocalIcon, local_icon},
    search::{SuggestionKind, SuggestionRow},
    theme::{BORDER, MUTED, SURFACE_RAISED},
};

use super::{Nav, RalgrumApp};

pub(super) fn render_search_suggestions(
    rows: Vec<SuggestionRow>,
    selected: Option<usize>,
    cx: &mut Context<RalgrumApp>,
) -> AnyElement {
    let items = rows
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            let query = row.query;
            let choose_query = query.clone();
            let icon = match row.kind {
                SuggestionKind::History => LocalIcon::ClockRotateLeft,
                SuggestionKind::SoundCloud => LocalIcon::MagnifyingGlass,
            };
            let history_remove = (row.kind == SuggestionKind::History).then(|| {
                let remove_query = query.clone();
                plain_x_button(
                    ("remove-search-history", index),
                    format!("remove-search-history-{index}"),
                    "Remove from recent searches",
                    false,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.search.update(cx, |search, cx| {
                            search.remove_search_history(&remove_query, cx)
                        });
                    }),
                )
            });
            div()
                .id(("search-suggestion", index))
                .h(px(38.))
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(10.))
                .rounded(px(6.))
                .role(Role::ListBoxOption)
                .aria_label(query.clone())
                .cursor_pointer()
                .when(selected == Some(index), |style| style.bg(rgba(0xffffff12)))
                .hover(|style| style.bg(rgba(0xffffff0d)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.external_detail_return = None;
                        this.nav = Nav::Discover;
                        this.search.update(cx, |search, cx| {
                            search.choose_suggestion(choose_query.clone(), window, cx)
                        });
                    }),
                )
                .child(local_icon(icon, MUTED).size(px(13.)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(13.))
                        .whitespace_nowrap()
                        .truncate()
                        .child(query),
                )
                .children(history_remove)
        })
        .collect::<Vec<_>>();

    deferred(
        div()
            .id("search-suggestions")
            .absolute()
            .top(px(42.))
            .left_0()
            .right_0()
            .p(px(4.))
            .border_1()
            .border_color(rgb(BORDER))
            .rounded(px(8.))
            .bg(rgb(SURFACE_RAISED))
            .shadow_lg()
            .occlude()
            .role(Role::ListBox)
            .aria_label("Search suggestions")
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.search
                    .update(cx, |search, cx| search.dismiss_suggestions(cx));
            }))
            .children(items),
    )
    .with_priority(1)
    .into_any_element()
}
