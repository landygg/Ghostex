//! The phone's copy of the desktop's `native_chat/visual_popup.rs`.
//!
//! CDXC:SessionChat 2026-10-09 WHY: The GPUI chat on the phone has no web runtime to float an agent's HTML page in, so a page card opens it as a link, which the phone shows in its web preview.

pub(super) const FLOATING_PAGES: bool = false;
