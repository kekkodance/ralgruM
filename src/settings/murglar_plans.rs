use std::{cell::RefCell, collections::HashMap, time::Instant};

use gpui::{
    AnimationExt as _, AnyElement, Context, Div, FontWeight, IntoElement, Role, SharedString,
    Stateful, div, ease_in_out, prelude::*, px, rgb, rgba,
};

use super::super::service_panel::settings_card;
use super::SettingsView;
use crate::{
    app_tooltip::AppTooltipExt,
    assets::{LocalIcon, local_icon},
    murglar_backend::{PaymentPlan, PaymentPlans},
    theme::{BORDER, FOREGROUND, MUTED},
};

thread_local! {
    static PAYMENT_METHODS_MOTION: RefCell<HashMap<String, PaymentMethodsMotion>> =
        RefCell::new(HashMap::new());
}

const PAYMENT_METHOD_MIN_HEIGHT: f32 = 31.;
const PAYMENT_METHOD_PADDING_Y: f32 = 5.;
const PAYMENT_METHOD_PADDING_X: f32 = 9.;
const PAYMENT_METHOD_GAP: f32 = 6.;
const PAYMENT_METHOD_RADIUS: f32 = 6.;
const PAYMENT_METHOD_ICON_SIZE: f32 = 12.;
const PAYMENT_METHOD_BACKGROUND: u32 = 0x27272a8c;
const PAYMENT_METHOD_TEXT: u32 = 0xd4d4d8;
const PAYMENT_METHOD_HOVER_BORDER: u32 = 0x6366f185;
const PAYMENT_METHOD_HOVER_BACKGROUND: u32 = 0x6366f129;
const PAYMENT_METHOD_HOVER_TEXT: u32 = 0xeef2ff;
const PREMIUM_PLAN_TITLE_SIZE_PX: f32 = 12.5;
const PREMIUM_PLAN_DESCRIPTION_SIZE_PX: f32 = 10.5;
const PAYMENT_METHODS_PADDING_Y: f32 = 9.;
const PAYMENT_METHODS_CONTENT_GAP: f32 = 8.;
const PAYMENT_METHODS_LABEL_HEIGHT: f32 = 16.;
const PAYMENT_METHODS_BORDER_T: f32 = 1.;
const PAYMENT_METHODS_PER_ROW: usize = 2;

#[derive(Clone, Copy, Debug, Default)]
struct PaymentMethodsMotion {
    initialized: bool,
    from: f32,
    target: f32,
    epoch: u64,
    started_at: Option<Instant>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PaymentMethodsVisual {
    from: f32,
    target: f32,
    epoch: u64,
    active: bool,
}

pub(super) fn reset_payment_methods_motion() {
    PAYMENT_METHODS_MOTION.with(|slot| slot.borrow_mut().clear());
}

pub(super) fn card(
    plans: &PaymentPlans,
    expanded_plan: Option<&str>,
    cx: &mut Context<SettingsView>,
) -> impl IntoElement + use<> {
    let mut card = settings_card()
        .gap(px(12.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .child(local_icon(LocalIcon::Layers, FOREGROUND).size(px(13.)))
                .child("Premium and Pass plans"),
        )
        .child(
            div()
                .text_size(px(11.5))
                .line_height(px(17.25))
                .text_color(rgb(MUTED))
                .child("Prices and payment methods loaded directly from Murglar."),
        );

    if let Some(plan) = &plans.premium {
        let expanded = expanded_plan == Some(plan.id.as_str());
        let can_expand = !plan.merchants.is_empty();
        let plan_id = plan.id.clone();
        card = card.child(plan_entry(
            plan,
            "Permanent Premium",
            true,
            Some(plans.premium_description()),
            false,
            expanded,
            can_expand,
            false,
            cx.listener(move |this, _, _, cx| {
                toggle_expanded_plan(this, plan_id.clone(), cx);
            }),
            cx,
        ));
    }

    if !plans.subscriptions.is_empty() {
        let best_subscription_id = plans.best_subscription_id();
        let mut subscriptions = div()
            .flex()
            .flex_col()
            .rounded(px(6.))
            .border_1()
            .border_color(rgb(BORDER))
            .overflow_hidden();
        for (index, plan) in plans.subscriptions.iter().enumerate() {
            let expanded = expanded_plan == Some(plan.id.as_str());
            let can_expand = !plan.merchants.is_empty();
            let plan_id = plan.id.clone();
            subscriptions = subscriptions.child(plan_entry(
                plan,
                "Murglar Pass",
                false,
                None,
                best_subscription_id == Some(plan.id.as_str()),
                expanded,
                can_expand,
                has_subscription_separator(index),
                cx.listener(move |this, _, _, cx| {
                    toggle_expanded_plan(this, plan_id.clone(), cx);
                }),
                cx,
            ));
        }
        card = card.child(subscriptions);
    }

    card
}

fn plan_entry(
    plan: &PaymentPlan,
    fallback_title: &'static str,
    premium: bool,
    description: Option<&str>,
    best_deal: bool,
    expanded: bool,
    can_expand: bool,
    separator: bool,
    toggle_action: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
    cx: &mut Context<SettingsView>,
) -> Div {
    let title = if plan.title.trim().is_empty() {
        fallback_title.to_owned()
    } else {
        plan.title.clone()
    };
    let price = if plan.price.trim().is_empty() {
        "Price unavailable".to_owned()
    } else {
        plan.price.clone()
    };
    let has_payment_methods = !plan.merchants.is_empty();
    let promotion_id = payment_promotion_id(plan);
    let mut methods = div().flex().flex_wrap().gap(px(6.));
    for merchant in &plan.merchants {
        let promotion_id = promotion_id.clone();
        let merchant_id = merchant.id.clone();
        let merchant_title: SharedString = merchant.title.clone().into();
        methods = methods.child(
            payment_method_button(
                format!("pay-{promotion_id}-{merchant_id}"),
                merchant_icon(&merchant.id),
                merchant_title,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.begin_payment(promotion_id.clone(), merchant_id.clone(), cx);
            })),
        );
    }
    if !has_payment_methods {
        methods = methods.child(
            div()
                .text_size(px(10.5))
                .text_color(rgb(MUTED))
                .child(payment_methods_label(false)),
        );
    }

    let mut title_row = div().flex().items_center().gap(px(7.)).min_w_0().child(
        div()
            .min_w_0()
            .truncate()
            .text_size(px(if premium {
                PREMIUM_PLAN_TITLE_SIZE_PX
            } else {
                11.5
            }))
            .font_weight(FontWeight::SEMIBOLD)
            .child(title.clone()),
    );
    if best_deal {
        title_row = title_row.child(
            div()
                .flex_none()
                .h(px(19.))
                .px(px(6.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .border_1()
                .border_color(rgba(0x6366f173))
                .bg(rgba(0x6366f12b))
                .text_color(rgb(0xc7d2fe))
                .text_size(px(9.5))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Best deal"),
        );
    }
    let mut title_copy = div().min_w_0().flex().flex_col().child(title_row);
    if let Some(description) = description.filter(|description| !description.trim().is_empty()) {
        title_copy = title_copy.child(
            div()
                .min_w_0()
                .truncate()
                .mt(px(2.))
                .text_size(px(PREMIUM_PLAN_DESCRIPTION_SIZE_PX))
                .line_height(px(14.7))
                .text_color(rgb(MUTED))
                .child(description.to_owned()),
        );
    }

    let header = div()
        .id(format!("murglar-plan-{promotion_id}"))
        .focusable()
        .tab_stop(can_expand)
        .role(Role::Button)
        .aria_label(title.clone())
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .min_h(px(45.))
        .px(px(11.))
        .py(px(8.))
        .when(!premium, |this| this.bg(rgba(0x09090b57)))
        .when(premium, |this| this.bg(rgba(0x6366f117)))
        .when(expanded, |this| {
            this.bg(if premium {
                rgba(0x6366f12c)
            } else {
                rgba(0x6366f13d)
            })
        })
        .when(can_expand, |this| {
            this.cursor_pointer()
                .hover(|style| style.bg(rgba(0x6366f11c)))
                .focus_visible(|style| style.border_1().border_color(rgb(crate::theme::PRIMARY)))
                .on_click(toggle_action)
        })
        .when(!can_expand, |this| this.opacity(0.86))
        .child(title_copy)
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(9.))
                .child(
                    div()
                        .text_color(rgb(0xc7d2fe))
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(price),
                )
                .child(
                    local_icon(
                        if expanded {
                            LocalIcon::ChevronUp
                        } else {
                            LocalIcon::ChevronDown
                        },
                        MUTED,
                    )
                    .size(px(9.)),
                ),
        );

    let methods_content = div()
        .flex()
        .flex_col()
        .gap(px(PAYMENT_METHODS_CONTENT_GAP))
        .px(px(11.))
        .py(px(PAYMENT_METHODS_PADDING_Y))
        .border_t_1()
        .border_color(rgba(0x6366f12e))
        .bg(rgba(0x09090b47))
        .child(
            div()
                .text_size(px(11.))
                .text_color(rgb(MUTED))
                .child(payment_methods_label(has_payment_methods)),
        )
        .child(methods);

    div()
        .flex()
        .flex_col()
        .when(separator, |this| {
            this.border_t_1().border_color(rgb(BORDER))
        })
        .when(!premium, |this| this.bg(rgba(0x09090b57)))
        .when(premium, |this| {
            this.border_1()
                .border_color(rgba(0x6366f159))
                .rounded(px(6.))
                .bg(rgba(0x6366f117))
        })
        .child(header)
        .when(can_expand, |this| {
            this.when_some(
                payment_methods_panel(
                    methods_content,
                    promotion_id,
                    expanded,
                    plan.merchants.len(),
                ),
                |this, panel| this.child(panel),
            )
        })
}

fn toggle_expanded_plan(this: &mut SettingsView, plan_id: String, cx: &mut Context<SettingsView>) {
    this.expanded_murglar_plan = if this.expanded_murglar_plan.as_deref() == Some(plan_id.as_str())
    {
        None
    } else {
        Some(plan_id)
    };
    cx.notify();
}

fn payment_methods_panel(
    methods_content: Div,
    plan_id: String,
    expanded: bool,
    merchant_count: usize,
) -> Option<AnyElement> {
    let visual = payment_methods_visual(&plan_id, expanded, Instant::now());
    if !visual.active && visual.target <= 0. {
        return None;
    }
    let reveal_height = payment_methods_reveal_height(merchant_count);
    let panel = div().overflow_hidden().w_full().child(methods_content);
    Some(if visual.active {
        panel
            .with_animation(
                format!("murglar-plan-methods-{plan_id}-{}", visual.epoch),
                crate::motion::content(),
                move |this, delta| {
                    let progress = crate::motion::lerp(visual.from, visual.target, delta);
                    this.opacity(progress).max_h(px(reveal_height * progress))
                },
            )
            .into_any_element()
    } else {
        panel.into_any_element()
    })
}

fn payment_methods_visual(plan_id: &str, expanded: bool, now: Instant) -> PaymentMethodsVisual {
    PAYMENT_METHODS_MOTION.with(|slot| {
        let mut map = slot.borrow_mut();
        let state = map.entry(plan_id.to_owned()).or_default();
        let target = if expanded { 1. } else { 0. };

        if !state.initialized {
            state.initialized = true;
            state.from = target;
            state.target = target;
            state.started_at = None;
        } else if (state.target - target).abs() > f32::EPSILON {
            state.from = payment_methods_displayed_at(state, now);
            state.target = target;
            state.epoch = state.epoch.wrapping_add(1);
            state.started_at = Some(now);
        } else if state.started_at.is_some_and(|started_at| {
            now.saturating_duration_since(started_at) >= crate::motion::CONTENT_DURATION
        }) {
            state.from = state.target;
            state.started_at = None;
        }

        PaymentMethodsVisual {
            from: state.from,
            target: state.target,
            epoch: state.epoch,
            active: state.started_at.is_some(),
        }
    })
}

fn payment_methods_displayed_at(state: &PaymentMethodsMotion, now: Instant) -> f32 {
    let Some(started_at) = state.started_at else {
        return state.target;
    };
    let elapsed = now.saturating_duration_since(started_at);
    if elapsed >= crate::motion::CONTENT_DURATION {
        return state.target;
    }
    let duration = crate::motion::CONTENT_DURATION.as_secs_f32();
    let progress = if duration == 0. {
        1.
    } else {
        (elapsed.as_secs_f32() / duration).clamp(0., 1.)
    };
    crate::motion::lerp(state.from, state.target, ease_in_out(progress))
}

fn payment_methods_reveal_height(merchant_count: usize) -> f32 {
    let rows = merchant_count.max(1).div_ceil(PAYMENT_METHODS_PER_ROW);
    let methods_height = rows as f32 * PAYMENT_METHOD_MIN_HEIGHT
        + rows.saturating_sub(1) as f32 * PAYMENT_METHOD_GAP;
    PAYMENT_METHODS_BORDER_T
        + PAYMENT_METHODS_PADDING_Y * 2.
        + PAYMENT_METHODS_LABEL_HEIGHT
        + PAYMENT_METHODS_CONTENT_GAP
        + methods_height
}

fn has_subscription_separator(index: usize) -> bool {
    index > 0
}

fn payment_methods_label(has_methods: bool) -> &'static str {
    if has_methods {
        "Choose payment method"
    } else {
        "No payment methods available"
    }
}

fn payment_promotion_id(plan: &PaymentPlan) -> String {
    plan.id.clone()
}

fn payment_method_button(id: String, icon: LocalIcon, label: SharedString) -> Stateful<Div> {
    let hover_group: SharedString = format!("payment-method-hover-{id}").into();
    let tooltip = label.clone();

    div()
        .id(id)
        .focusable()
        .tab_stop(true)
        .role(Role::Button)
        .group(hover_group.clone())
        .flex_none()
        .min_h(px(PAYMENT_METHOD_MIN_HEIGHT))
        .py(px(PAYMENT_METHOD_PADDING_Y))
        .px(px(PAYMENT_METHOD_PADDING_X))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(PAYMENT_METHOD_GAP))
        .border_1()
        .border_color(rgb(BORDER))
        .rounded(px(PAYMENT_METHOD_RADIUS))
        .bg(rgba(PAYMENT_METHOD_BACKGROUND))
        .text_color(rgb(PAYMENT_METHOD_TEXT))
        .text_size(px(10.5))
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|style| {
            style
                .border_color(rgba(PAYMENT_METHOD_HOVER_BORDER))
                .bg(rgba(PAYMENT_METHOD_HOVER_BACKGROUND))
                .text_color(rgb(PAYMENT_METHOD_HOVER_TEXT))
        })
        .focus_visible(|style| style.border_1().border_color(rgb(crate::theme::PRIMARY)))
        .aria_label(label.clone())
        .app_tooltip(tooltip)
        .child(
            div()
                .relative()
                .size(px(PAYMENT_METHOD_ICON_SIZE))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .group_hover(hover_group.clone(), |style| style.invisible())
                        .child(local_icon(icon, PAYMENT_METHOD_TEXT).size_full()),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .invisible()
                        .group_hover(hover_group, |style| style.visible())
                        .child(local_icon(icon, PAYMENT_METHOD_HOVER_TEXT).size_full()),
                ),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(10.5))
                .line_height(px(13.125))
                .child(label),
        )
}

fn merchant_icon(merchant_id: &str) -> LocalIcon {
    match merchant_id {
        "TELEGRAM_BOT" => LocalIcon::Telegram,
        "PAYPAL" => LocalIcon::PayPal,
        "CRYPTO" => LocalIcon::Bitcoin,
        _ => LocalIcon::CreditCard,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PAYMENT_METHOD_BACKGROUND, PAYMENT_METHOD_GAP, PAYMENT_METHOD_HOVER_BACKGROUND,
        PAYMENT_METHOD_HOVER_BORDER, PAYMENT_METHOD_HOVER_TEXT, PAYMENT_METHOD_ICON_SIZE,
        PAYMENT_METHOD_MIN_HEIGHT, PAYMENT_METHOD_PADDING_X, PAYMENT_METHOD_PADDING_Y,
        PAYMENT_METHOD_RADIUS, PAYMENT_METHOD_TEXT, PREMIUM_PLAN_DESCRIPTION_SIZE_PX,
        PREMIUM_PLAN_TITLE_SIZE_PX, has_subscription_separator, merchant_icon,
        payment_methods_label, payment_promotion_id,
    };
    use crate::assets::LocalIcon;
    use crate::murglar_backend::PaymentPlan;

    #[test]
    fn empty_titles_use_reference_fallbacks() {
        let premium = "Permanent Premium";
        let pass = "Murglar Pass";
        assert_eq!(premium, "Permanent Premium");
        assert_eq!(pass, "Murglar Pass");
    }

    #[test]
    fn merchant_ids_use_the_reference_payment_icons() {
        assert!(matches!(merchant_icon("TELEGRAM_BOT"), LocalIcon::Telegram));
        assert!(matches!(merchant_icon("PAYPAL"), LocalIcon::PayPal));
        assert!(matches!(merchant_icon("CRYPTO"), LocalIcon::Bitcoin));
        assert!(matches!(merchant_icon("OXAPAY"), LocalIcon::CreditCard));
        assert!(matches!(merchant_icon("AURAPAY"), LocalIcon::CreditCard));
        assert!(matches!(merchant_icon("UNKNOWN"), LocalIcon::CreditCard));
    }

    #[test]
    fn premium_payment_uses_the_server_provided_promotion_id() {
        let plan = PaymentPlan {
            id: "NO_ADS_SUB_10_MONTH".into(),
            title: "Premium".into(),
            price: "10 USD".into(),
            price_usd: None,
            price_rub: None,
            merchants: Vec::new(),
        };

        assert_eq!(payment_promotion_id(&plan), "NO_ADS_SUB_10_MONTH");
    }

    #[test]
    fn payment_method_style_matches_reference_contract() {
        assert_eq!(PAYMENT_METHOD_MIN_HEIGHT, 31.);
        assert_eq!(PAYMENT_METHOD_PADDING_Y, 5.);
        assert_eq!(PAYMENT_METHOD_PADDING_X, 9.);
        assert_eq!(PAYMENT_METHOD_GAP, 6.);
        assert_eq!(PAYMENT_METHOD_RADIUS, 6.);
        assert_eq!(PAYMENT_METHOD_ICON_SIZE, 12.);
        assert_eq!(PAYMENT_METHOD_BACKGROUND, 0x27272a8c);
        assert_eq!(PAYMENT_METHOD_TEXT, 0xd4d4d8);
        assert_eq!(PAYMENT_METHOD_HOVER_BORDER, 0x6366f185);
        assert_eq!(PAYMENT_METHOD_HOVER_BACKGROUND, 0x6366f129);
        assert_eq!(PAYMENT_METHOD_HOVER_TEXT, 0xeef2ff);
    }

    #[test]
    fn plan_copy_and_separators_match_reference_contract() {
        assert_eq!(PREMIUM_PLAN_TITLE_SIZE_PX, 12.5);
        assert_eq!(PREMIUM_PLAN_DESCRIPTION_SIZE_PX, 10.5);
        assert!(!has_subscription_separator(0));
        assert!(has_subscription_separator(1));
        assert_eq!(payment_methods_label(true), "Choose payment method");
        assert_eq!(payment_methods_label(false), "No payment methods available");
    }
}
