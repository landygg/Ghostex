//! The browser build's copy of the desktop's `native_chat/visual_popup.rs`.
//!
//! CDXC:SessionChat 2026-10-09 WHY: The page has no web runtime to float an agent's HTML page in, so a page card opens it in a new browser tab, where it runs in the same sandbox.

pub(super) const FLOATING_PAGES: bool = false;
