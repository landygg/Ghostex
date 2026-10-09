//! Drawing of the Add Project dialog's clone review step: the repository and destination cards,
//! the optional Git settings (branch, branch only, shallow) and the Back / Clone & Add footer.
use super::super::native_modal_kit::*;
use super::paths::is_repository_clone_branch_name_input_valid;
use super::skin::*;
use super::window::*;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use gpui_component::input::Input;
use gpui_component::{Sizable as _, Size as ComponentSize, h_flex, v_flex};

/// `leading-relaxed` on 14px text.
const RELAXED_LINE: f32 = 22.75;
/// Below this the two clone option cards stack instead of sharing the row.
const OPTION_CARD_MIN_WIDTH: f32 = 220.0;

impl GpuiAddProjectModalWindow {
    /// A review card (`border border-border/60 bg-muted/15 px-3 py-2.5`): icon, label, a
    /// middle-ellipsis value and optional lines under it.
    fn review_card(
        &self,
        skin: &AddProjectSkin,
        icon: &'static str,
        label: &'static str,
        value: &str,
        extra: Vec<AnyElement>,
        blocked: bool,
    ) -> AnyElement {
        h_flex()
            .min_w_0()
            .items_start()
            .gap(px(10.0))
            .px(px(12.0))
            .py(px(10.0))
            .border_1()
            .border_color(hsla(if blocked {
                css_fade(skin.destructive, 0.5)
            } else {
                skin.border_at(0.6)
            }))
            .bg(hsla(skin.muted_fill_at(0.15)))
            .child(div().flex_shrink_0().mt(px(2.0)).child(modal_icon(
                icon,
                16.0,
                skin.muted_at(0.8),
            )))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(label_medium(skin, label))
                    .child(
                        text_sm(middle_ellipsis(value))
                            .mt(px(4.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(hsla(skin.fg())),
                    )
                    .children(extra),
            )
            .into_any_element()
    }

    /// A checkbox card of the clone options (`label` wrapping the checkbox, title and hint).
    fn option_card(
        &mut self,
        skin: &AddProjectSkin,
        slot: FocusSlot,
        checked: bool,
        title: &'static str,
        description: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let handle = self.chrome_focus_handle(slot, cx);
        let focused = handle.is_focused(window);
        let disabled = self.busy.is_some();
        let hover = skin.muted_fill_at(0.3);
        let main_only = slot == FocusSlot::CloneMainOnly;
        h_flex()
            .id(match slot {
                FocusSlot::CloneMainOnly => "add-project-clone-main-only",
                _ => "add-project-shallow-clone",
            })
            .track_focus(&handle)
            .flex_1()
            .flex_basis(px(OPTION_CARD_MIN_WIDTH))
            .min_w_0()
            .items_start()
            .gap(px(10.0))
            .px(px(12.0))
            .py(px(10.0))
            .border_1()
            .border_color(hsla(skin.border_at(0.5)))
            .hover(move |this| this.bg(hsla(hover)))
            .when(!disabled, |this| {
                this.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.set_clone_option(main_only, window, cx);
                }))
            })
            .child(
                div()
                    .flex_shrink_0()
                    .mt(px(2.0))
                    .when(disabled, |this| this.opacity(0.5))
                    .child(square_checkbox(skin, checked, focused)),
            )
            // Without `flex_1` the column collapses to zero width and wraps one letter per line.
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        text_sm(div())
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(hsla(skin.fg()))
                            .child(title),
                    )
                    .child(
                        div()
                            .mt(px(2.0))
                            .text_size(px(14.0))
                            .line_height(px(RELAXED_LINE))
                            .text_color(hsla(skin.muted()))
                            .child(description),
                    ),
            )
            .into_any_element()
    }

    /// A shadcn footer button inside the dialog: 32px, 8px radius, 14px/400. `primary` is the
    /// filled default variant, else the outline one (muted label, `--background` fill).
    #[allow(clippy::too_many_arguments)]
    fn footer_button(
        &mut self,
        skin: &AddProjectSkin,
        slot: FocusSlot,
        label: &'static str,
        primary: bool,
        disabled: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let handle = self.chrome_focus_handle(slot, cx);
        let focused = handle.is_focused(window) && !disabled;
        let p = skin.p;
        let (background, border, text, hover_background, hover_text) = if primary {
            (
                p.primary,
                css_fade(p.primary, 0.0),
                p.primary_foreground,
                css_fade(p.primary, 0.8),
                p.primary_foreground,
            )
        } else {
            (
                p.surface,
                skin.border,
                skin.muted(),
                skin.muted_fill,
                p.foreground,
            )
        };
        let button = h_flex()
            .id(match slot {
                FocusSlot::FooterClone => "add-project-clone",
                _ => "add-project-review-back",
            })
            .track_focus(&handle)
            .flex_shrink_0()
            .h(px(32.0))
            .px(px(12.0))
            .items_center()
            .justify_center()
            .rounded(px(8.0))
            .border_1()
            .border_color(hsla(border))
            .bg(hsla(background))
            .text_size(px(14.0))
            .line_height(px(20.0))
            .text_color(hsla(text))
            .whitespace_nowrap()
            .when(disabled, |this| this.opacity(0.5))
            .when(!disabled, |this| {
                this.hover(move |this| this.bg(hsla(hover_background)).text_color(hsla(hover_text)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        on_click(this, window, cx);
                    }))
            })
            .child(label);
        skin.focus_ring(button, focused).into_any_element()
    }

    fn render_branch_field(
        &self,
        skin: &AddProjectSkin,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let invalid = !is_repository_clone_branch_name_input_valid(&self.clone_options.branch_name);
        let focused =
            gpui::Focusable::focus_handle(self.branch_input.read(cx), cx).is_focused(window);
        let disabled = self.busy.is_some();
        let p = skin.p;
        let field = div()
            .w_full()
            .h(px(36.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .border_1()
            .border_color(hsla(skin.input))
            .bg(hsla(skin.on_options_panel(css_fade(skin.input, 0.3))))
            .when(disabled, |this| this.opacity(0.5))
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&self.branch_input)
                        .with_size(ComponentSize::Small)
                        .appearance(false)
                        .bordered(false)
                        .focus_bordered(false)
                        .disabled(disabled)
                        .w_full()
                        .px(px(0.0))
                        .py(px(0.0))
                        .text_size(px(14.0))
                        .text_color(hsla(p.foreground)),
                ),
            );
        let field = if invalid {
            // `aria-invalid`: the destructive edge and halo, focused or not.
            field
                .border_color(hsla(skin.destructive))
                .shadow(skin.ring_halo(skin.destructive))
        } else {
            skin.focus_ring(field, focused)
        };
        v_flex()
            .min_w_0()
            .child(
                h_flex()
                    .mb(px(6.0))
                    .items_center()
                    .gap(px(6.0))
                    .text_size(px(14.0))
                    .line_height(px(20.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(hsla(skin.muted()))
                    .child(modal_icon(ICON_GIT_BRANCH, 14.0, skin.muted()))
                    .child("Branch"),
            )
            .child(field)
            .child(
                text_sm(div())
                    .mt(px(6.0))
                    .text_color(hsla(if invalid {
                        skin.destructive
                    } else {
                        skin.muted()
                    }))
                    .child(if invalid {
                        "Enter a valid Git branch name."
                    } else {
                        "Leave empty to use the repository default branch."
                    }),
            )
            .into_any_element()
    }

    pub(super) fn render_review(
        &mut self,
        skin: &AddProjectSkin,
        d: &Derived,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (Some(flow), Some(preview)) = (self.clone_flow.clone(), self.clone_preview.clone())
        else {
            return div().into_any_element();
        };
        let busy = self.busy.is_some();
        let back_handle = self.chrome_focus_handle(FocusSlot::ReviewBack, cx);
        let back_focused = back_handle.is_focused(window) && !busy;
        let muted_fill = skin.muted_fill;
        let back = skin.focus_ring(
            div()
                .id("add-project-review-arrow")
                .track_focus(&back_handle)
                .flex_shrink_0()
                .mt(px(2.0))
                .size(px(32.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.0))
                .border_1()
                .border_color(gpui::transparent_black())
                .when(back_focused, |this| this.bg(hsla(skin.p.surface)))
                .when(busy, |this| this.opacity(0.5))
                .when(!busy, |this| {
                    this.hover(move |this| this.bg(hsla(muted_fill)))
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.return_to_clone_destination(window, cx);
                        }))
                })
                .child(modal_icon(ICON_ARROW_LEFT, 16.0, skin.fg())),
            back_focused,
        );
        let header = h_flex()
            .min_w_0()
            .items_start()
            .gap(px(12.0))
            .child(back)
            .child(
                v_flex()
                    .min_w_0()
                    .child(
                        text_sm(div())
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(hsla(skin.fg()))
                            .child("Review clone"),
                    )
                    .child(
                        div()
                            .mt(px(2.0))
                            .text_size(px(14.0))
                            .line_height(px(RELAXED_LINE))
                            .text_color(hsla(skin.muted()))
                            .child("Confirm the destination and adjust optional Git settings."),
                    ),
            )
            .children(d.machine.as_ref().map(|machine| {
                h_flex()
                    .ml_auto()
                    .flex_shrink_0()
                    .pt(px(4.0))
                    .items_center()
                    .gap(px(6.0))
                    .text_color(hsla(skin.muted()))
                    .child(modal_icon(ICON_FOLDER_PLUS, 12.0, skin.muted()))
                    .child(
                        div()
                            .max_w(px(128.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(machine.label.clone()),
                    )
            }));
        let repository_name = flow
            .repository
            .as_ref()
            .map(|repository| repository.name_with_owner.clone())
            .unwrap_or_else(|| flow.repository_input.clone());
        let repository_url = flow
            .repository
            .as_ref()
            .map(|repository| repository.url.clone())
            .unwrap_or_else(|| flow.remote_url.clone());
        let repository_card = self.review_card(
            skin,
            source_icon(flow.source),
            "Repository",
            &repository_name,
            vec![
                text_sm(middle_ellipsis(&repository_url))
                    .mt(px(2.0))
                    .text_color(hsla(skin.muted()))
                    .into_any_element(),
            ],
            false,
        );
        let destination_description =
            if preview.destination_blocked {
                Some(preview.warning.clone().unwrap_or_else(|| {
                    "Choose a different destination before cloning.".to_string()
                }))
            } else if preview.destination_exists && preview.destination_is_empty == Some(true) {
                Some(
                    "Existing empty folder. The repository will be cloned directly into it."
                        .to_string(),
                )
            } else {
                None
            };
        let destination_card = self.review_card(
            skin,
            ICON_FOLDER_CHECK,
            "Destination",
            &preview.destination_path,
            destination_description
                .map(|description| {
                    div()
                        .mt(px(2.0))
                        .text_size(px(14.0))
                        .line_height(px(RELAXED_LINE))
                        .text_color(hsla(if preview.destination_blocked {
                            skin.destructive
                        } else {
                            skin.muted()
                        }))
                        .child(description)
                        .into_any_element()
                })
                .into_iter()
                .collect(),
            preview.destination_blocked,
        );
        let branch_field = self.render_branch_field(skin, window, cx);
        let main_only = self.option_card(
            skin,
            FocusSlot::CloneMainOnly,
            self.clone_options.clone_main_only,
            "Clone branch only",
            "Fetch only the selected branch.",
            window,
            cx,
        );
        let shallow = self.option_card(
            skin,
            FocusSlot::ShallowClone,
            self.clone_options.shallow_clone,
            "Shallow clone",
            "Fetch only the latest commit history.",
            window,
            cx,
        );
        let options = v_flex()
            .p(px(12.0))
            .border_1()
            .border_color(hsla(skin.border_at(0.6)))
            .bg(hsla(skin.muted_fill_at(0.1)))
            .child(
                h_flex()
                    .mb(px(12.0))
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .child(
                        text_sm(div())
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(hsla(skin.fg()))
                            .child("Clone options"),
                    )
                    .child(label_medium(skin, "Optional")),
            )
            .child(branch_field)
            .child(
                h_flex()
                    .mt(px(12.0))
                    .flex_wrap()
                    .gap(px(8.0))
                    .items_stretch()
                    .child(main_only)
                    .child(shallow),
            );
        // git's own progress line once the job reports one; the slow notice before that.
        let clone_notice = match (self.busy == Some(Busy::Clone), self.clone_progress.clone()) {
            (true, Some(progress)) => Some(self.render_slow_notice(skin, progress, cx)),
            (true, None) if self.is_slow => Some(self.render_slow_notice(
                skin,
                "Still cloning. The machine may be reconnecting.",
                cx,
            )),
            _ => None,
        };
        let content = v_flex()
            .w_full()
            .flex_shrink_0()
            .gap(px(16.0))
            .child(header)
            .child(
                v_flex()
                    .min_w_0()
                    .gap(px(8.0))
                    .child(repository_card)
                    .child(destination_card),
            )
            .child(options)
            .children(
                self.error
                    .as_ref()
                    .map(|error| error_region(skin, error).into_any_element()),
            )
            .children(clone_notice);
        let can_clone = self.can_clone();
        let cloning = self.busy == Some(Busy::Clone);
        let back_button = self.footer_button(
            skin,
            FocusSlot::FooterBack,
            "Back",
            false,
            busy,
            |this, window, cx| this.return_to_clone_destination(window, cx),
            window,
            cx,
        );
        let clone_button = self.footer_button(
            skin,
            FocusSlot::FooterClone,
            if cloning { "Cloning..." } else { "Clone & Add" },
            true,
            !can_clone,
            |this, window, cx| this.submit_clone(window, cx),
            window,
            cx,
        );
        let footer = h_flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(12.0))
            .border_t_1()
            .border_color(hsla(skin.border_at(0.7)))
            .px(px(12.0))
            .py(px(10.0))
            .text_size(px(14.0))
            .line_height(px(20.0))
            .text_color(hsla(skin.muted()))
            .child(footer_hint(skin, "Esc", "Close"))
            .child(
                h_flex()
                    .ml_auto()
                    .items_center()
                    .gap(px(8.0))
                    .child(back_button)
                    .child(clone_button),
            );
        v_flex()
            .size_full()
            .min_w_0()
            .child(
                div()
                    .id("add-project-review-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.review_scroll)
                    .px(px(12.0))
                    .py(px(16.0))
                    .child(content),
            )
            .child(footer)
            .into_any_element()
    }
}
