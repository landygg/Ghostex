//! The cards for messages between agents: one another agent sent this session, and one this
//! session's agent sent another. Which rows these are, and whom a message went to, comes from the
//! core (packages/gx-chat-core/src/transcript/agent_message.rs and sent_message.rs).

use super::cards::status_card_press_header;
use super::disclosure_motion::measured;
use super::thinking::estimated_lines;
use super::{appearance::ChatAppearance, state::NativeChatView, transcript::text};
use crate::app::native_chat::cursor::ChatCursor as _;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div, px,
};
use serde_json::Value;

/// Collapsed, a received message shows this many lines of its body.
const MESSAGE_PREVIEW_LINES: usize = 2;

/// What a message card's header says and how it sits on the card.
struct MessageHeader {
    key: String,
    title: String,
    detail: String,
    /// A short tag before the chevron ("QUEUED", "NOT SENT") and its colour.
    tag: Option<(&'static str, gpui::Hsla)>,
    expandable: bool,
    expanded: bool,
    /// Whether the card shows a body under the header, and a footer under that.
    has_body: bool,
    has_actions: bool,
}

impl NativeChatView {
    /// CDXC:SessionChat 2026-09-18 DECISION:
    /// User: a message another agent sent with `ghostex agents send` reads as a message from that agent, not as the user's own prompt bubble, and its sender header is never a heading.
    /// CDXC:SessionChat 2026-09-30 DECISION:
    /// User: "collapse the recieved message one to just show the first 2 lines by default but i can click to expand/collapse also". Collapsed, the card shows its body's first two lines; pressing the header or the preview opens the whole message, and the header closes it again.
    /// SEE-ALSO: packages/gx-chat-core/src/transcript/agent_message.rs parses the header; apps/mobile/app/src/chat/native/transcript/SystemRows.tsx draws the phone's card.
    pub(super) fn inter_agent_message_card(
        &self,
        id: &str,
        message: &Value,
        p: &ChatAppearance,
        cx: &Context<Self>,
    ) -> AnyElement {
        let sent = &message["interAgentMessage"];
        let body = text(sent, "body");
        let key = format!("inter-agent:{id}");
        let expandable = estimated_lines(&body) > MESSAGE_PREVIEW_LINES;
        let expanded = expandable && self.expanded.contains(&key);
        let motion = self.disclosure_frame(&key, expanded, cx);
        // React footed this card with the send's delivery status; a waiting send says so here too.
        let actions = self
            .render_startup_delivery(message, true, p, cx)
            .map_or_else(Vec::new, |status| vec![status]);
        let header = self.message_header(
            MessageHeader {
                key: key.clone(),
                title: format!("Message from {}", text(sent, "agentName")),
                detail: text(sent, "sessionTitle"),
                tag: (message["queued"] == true).then_some(("QUEUED", p.muted)),
                expandable,
                expanded,
                has_body: !body.is_empty(),
                has_actions: !actions.is_empty(),
            },
            p,
            cx,
        );
        let body = if body.is_empty() {
            Vec::new()
        } else if !expandable || expanded || motion.is_some() {
            let full = self.markdown(
                format!("inter-agent:{id}"),
                body,
                &message["markdownReferences"],
                p,
                cx,
            );
            vec![match motion {
                // Clipped between the two-line preview's height and the full text's.
                Some(frame) => self.capped_body_motion(&key, frame, 0.0, full),
                None => full,
            }]
        } else {
            // GPUI only clamps when the text also asks for an ellipsis.
            let toggle_key = key.clone();
            let preview = div()
                .id(format!("inter-agent-preview:{id}"))
                .min_w_0()
                .line_clamp(MESSAGE_PREVIEW_LINES)
                .text_ellipsis()
                .chat_cursor_pointer()
                .child(body)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_marker_disclosure(toggle_key.clone(), false, cx)
                }))
                .into_any_element();
            vec![measured(self.disclosure_floor(&key), preview)]
        };
        self.status_card_with_header(header, body, actions, p)
    }

    /// The cards for the messages one message's tool calls sent, or a finished turn lifted out of
    /// its work fold.
    ///
    /// CDXC:SessionChat 2026-09-30 DECISION:
    /// User: a message this session's agent sends another agent gets "a small card that shows the message being sent when expanded", "collapsed by default", in the received card's shape.
    /// SEE-ALSO: packages/gx-chat-core/src/transcript/sent_message.rs decides which calls these are and names the recipient; apps/mobile/app/src/chat/native/transcript/SystemRows.tsx draws the phone's card.
    pub(super) fn sent_agent_message_cards(
        &self,
        cards: &Value,
        p: &ChatAppearance,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let cards = cards.as_array().filter(|cards| !cards.is_empty())?;
        Some(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .w_full()
                .gap(px(8.0 * p.scale))
                .children(
                    cards
                        .iter()
                        .map(|card| self.sent_agent_message_card(card, p, cx)),
                )
                .into_any_element(),
        )
    }

    fn sent_agent_message_card(
        &self,
        card: &Value,
        p: &ChatAppearance,
        cx: &Context<Self>,
    ) -> AnyElement {
        let key = format!("sent-message:{}", text(card, "key"));
        let marked = text(card, "markdown");
        let body = if marked.is_empty() {
            text(card, "body")
        } else {
            marked
        };
        let expandable = !body.is_empty();
        let expanded = expandable && self.expanded.contains(&key);
        let motion = self.disclosure_frame(&key, expanded, cx);
        let has_body = expanded || motion.is_some();
        let header = self.message_header(
            MessageHeader {
                key: key.clone(),
                title: text(card, "title"),
                detail: text(card, "detail"),
                tag: (card["failed"] == true).then(|| ("NOT SENT", p.error())),
                expandable,
                expanded,
                has_body,
                has_actions: false,
            },
            p,
            cx,
        );
        let body = if has_body {
            vec![self.markdown(key.clone(), body, &card["markdownReferences"], p, cx)]
        } else {
            Vec::new()
        };
        self.status_card_with_header_motion(
            super::cards::CardBodyMotion {
                key: &key,
                frame: motion,
                shut_body: false,
                shut: !expanded,
            },
            header,
            body,
            Vec::new(),
            p,
        )
    }

    /// The message icon, the title with its session beside it, and the chevron; the whole header
    /// is the toggle when the card has more to show.
    fn message_header(
        &self,
        header: MessageHeader,
        p: &ChatAppearance,
        cx: &Context<Self>,
    ) -> AnyElement {
        let s = p.scale;
        let MessageHeader {
            key,
            title,
            detail,
            tag,
            expandable,
            expanded,
            has_body,
            has_actions,
        } = header;
        let row = div()
            .id(format!("{key}:header"))
            .flex()
            .items_start()
            .min_w_0()
            .gap(px(8.0 * s))
            .child(
                gpui::svg()
                    .path("titlebar/message.svg")
                    .size(px(14.0 * s))
                    .mt(px(4.0 * s))
                    .text_color(p.muted)
                    .flex_shrink_0(),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_wrap()
                    .gap_x(px(8.0 * s))
                    .child(div().text_color(p.foreground).child(title.clone()))
                    .when(!detail.is_empty(), |heading| {
                        heading.child(div().text_color(p.muted).child(detail))
                    }),
            )
            // On the title's first line, centred on it like the chevron.
            .when_some(tag, |row, (tag, color)| {
                row.child(
                    div()
                        .h(px(22.75 * s))
                        .flex()
                        .items_center()
                        .flex_shrink_0()
                        .text_size(px(11.0 * s))
                        .text_color(color)
                        .child(tag),
                )
            })
            .when(expandable, |row| {
                row.child(
                    div()
                        .h(px(22.75 * s))
                        .flex()
                        .items_center()
                        .flex_shrink_0()
                        .child(
                            gpui::svg()
                                .path(if expanded {
                                    "titlebar/chevron-down.svg"
                                } else {
                                    "titlebar/chevron-right.svg"
                                })
                                .size(px(14.0 * s))
                                .text_color(p.muted),
                        ),
                )
            });
        if !expandable {
            return row.into_any_element();
        }
        let row = row
            .role(gpui::Role::Button)
            .aria_label(title)
            .aria_expanded(expanded)
            .chat_cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_marker_disclosure(key.clone(), expanded, cx)
            }));
        status_card_press_header(row, has_body, has_actions, p).into_any_element()
    }
}
