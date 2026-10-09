//! The floating window a chat page card opens: an agent's published HTML page (`ghostex show`)
//! over the chat, closed by a click away, Escape or its close button.
//!
//! CDXC:SessionChat 2026-10-09 DECISION:
//! User: an agent's HTML page marked to open floating opens "in a pop up above the chat basically that when i click away goes away". A card whose block carries `"open": "popup"` opens its page here instead of in the browser.
//! CDXC:SessionChat 2026-10-09 WHY:
//! The page is loaded with `#gx-theme=light|dark`, which the bootstrap every published page starts with (server/src/visual_pages/theme.rs) reads before first paint, so the page wears the app's appearance rather than the system's. Links the page opens go to the reader's browser, so the window only ever shows the page itself.
//! SEE-ALSO: apps/desktop/src/app/visual_page_modal_lifecycle.rs (open and close), apps/desktop/src/app/native_chat/visual.rs (the card), packages/gx-visual (the `open` mark).
use super::native_modal_kit::*;
use crate::app::helpers::*;
use crate::*;
use gpui::{
    App, Context, Entity, FocusHandle, FontWeight, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, Styled as _, Window, div, point, px, size,
};
use gpui_component::{h_flex, v_flex};
use std::rc::Rc;

const HEADER_HEIGHT: f32 = 44.0;
const ICON_CLOSE: &str = "titlebar/x.svg";
const ICON_OPEN_IN_BROWSER: &str = "titlebar/external-link.svg";
/// Its own browser profile, so a page's storage never mixes with the Browser's or an
/// extension's.
const VISUAL_PAGE_CEF_PROFILE_ID: &str = "visual-pages";

pub(crate) enum VisualPageModalCommand {
    /// A click away, Escape or the close button.
    Close,
    /// Closes the window and opens the page in the reader's browser.
    OpenInBrowser { url: String },
}

pub(crate) type VisualPageModalHost = Rc<dyn Fn(VisualPageModalCommand, &mut App)>;

pub(crate) struct GpuiVisualPageModalWindow {
    host: VisualPageModalHost,
    palette: ModalPalette,
    url: String,
    title: String,
    detail: String,
    surface: Option<Entity<CefSurface>>,
    error: Option<String>,
    focus_handle: FocusHandle,
    _click_away: Vec<gpui::Subscription>,
}

impl GpuiVisualPageModalWindow {
    pub(crate) fn new(
        url: String,
        title: String,
        detail: String,
        palette: ModalPalette,
        host: VisualPageModalHost,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);
        let theme = if palette.light { "light" } else { "dark" };
        let page_url = format!("{}#gx-theme={theme}", url.split('#').next().unwrap_or(&url));
        let popup_open_handler: cef::BrowserPopupOpenHandler = Rc::new(|requested_url, _| {
            let _ = gpui_open_external_http_url(&requested_url);
        });
        let created = cef_parent_native_view(window)
            .map_err(|error| error.to_string())
            .and_then(|parent| {
                CefSurface::try_new(
                    "ghostex-gpui-visual-page".to_string(),
                    parent,
                    page_url,
                    VISUAL_PAGE_CEF_PROFILE_ID.to_string(),
                    pane_prepaint_background_color(),
                    false,
                    hsla(palette.surface),
                    None,
                    true,
                    Some(popup_open_handler),
                    None,
                    None,
                    None,
                    None,
                    None,
                    cx,
                )
            });
        let (surface, error) = match created {
            Ok(surface) => {
                // A CEF page starts 1×1 and only gets its frame when the window paints; give it
                // the area under the header from the start so it lays out at its real width.
                let viewport = window.viewport_size();
                surface.read(cx).set_initial_bounds(
                    gpui::Bounds::new(
                        point(px(0.0), px(HEADER_HEIGHT)),
                        size(
                            viewport.width,
                            (viewport.height - px(HEADER_HEIGHT)).max(px(1.0)),
                        ),
                    ),
                    window.scale_factor(),
                );
                (Some(surface), None)
            }
            Err(error) => {
                support_logs::append(
                    support_logs::GpuiSupportLog::CrashReports,
                    "gpui.cefSurface.createFailed",
                    serde_json::json!({ "surface": "visualPage", "error": error }),
                );
                (
                    None,
                    Some("This page could not be opened here.".to_string()),
                )
            }
        };
        Self {
            host,
            palette,
            url,
            title,
            detail,
            surface,
            error,
            focus_handle,
            _click_away: super::popup_dismissal::close_app_modal_on_click_away(window, cx),
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" {
            window.prevent_default();
            cx.stop_propagation();
            (self.host)(VisualPageModalCommand::Close, cx);
        }
    }
}

impl Render for GpuiVisualPageModalWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette.clone();
        let header = h_flex()
            .flex_shrink_0()
            .h(px(HEADER_HEIGHT))
            .w_full()
            .pl(px(14.0))
            .pr(px(6.0))
            .gap(px(4.0))
            .items_center()
            .border_b_1()
            .border_color(hsla(p.hairline))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hsla(p.foreground))
                            .child(self.title.clone()),
                    )
                    .when(!self.detail.is_empty(), |this| {
                        this.child(
                            div()
                                .truncate()
                                .text_size(px(11.0))
                                .text_color(hsla(p.muted))
                                .child(self.detail.clone()),
                        )
                    }),
            )
            .child(modal_icon_button(
                &p,
                "visual-page-open-in-browser",
                ICON_OPEN_IN_BROWSER,
                16.0,
                |this: &mut Self, _, cx| {
                    let url = this.url.clone();
                    (this.host)(VisualPageModalCommand::OpenInBrowser { url }, cx);
                },
                cx,
            ))
            .child(modal_icon_button(
                &p,
                "visual-page-close",
                ICON_CLOSE,
                16.0,
                |this: &mut Self, _, cx| (this.host)(VisualPageModalCommand::Close, cx),
                cx,
            ));
        let body = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .children(self.surface.clone())
            .when_some(self.error.clone(), |this, error| {
                this.flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(13.0))
                    .text_color(hsla(p.muted))
                    .child(error)
            });
        div()
            .id("ghostex-gpui-visual-page-modal")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(hsla(p.surface))
            .on_key_down(cx.listener(Self::on_key_down))
            .child(header)
            .child(body)
    }
}

impl ModalCornerClose for GpuiVisualPageModalWindow {
    fn close_from_corner(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        (self.host)(VisualPageModalCommand::Close, cx);
    }

    /// It draws its own close button in its header.
    fn shows_corner_close(&self, _cx: &App) -> bool {
        false
    }
}
