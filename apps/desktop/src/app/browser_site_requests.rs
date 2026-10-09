//! Answers what a web page asks of the computer (`cef/shell/site_requests.rs`): opening another app
//! through its link, and connecting to apps on this computer or devices on the local network. Each
//! is a native prompt in the window in front, the way Chrome asks.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{AnyWindowHandle, App, AsyncApp, PromptLevel};

use crate::cef::{BrowserExternalAppRequest, BrowserLocalNetworkAccessRequest, BrowserSiteRequest};

/// A page that asks again this soon after the user said Cancel is not asked, so a page retrying in
/// a loop cannot hold the window behind a prompt; a deliberate second click still asks.
const DECLINED_EXTERNAL_APP_QUIET_PERIOD: Duration = Duration::from_millis(1500);

thread_local! {
    /// GPUI prompts do not stack: while one of these is up, further requests are let go.
    static SITE_PROMPT_OPEN: Cell<bool> = const { Cell::new(false) };
    static DECLINED_EXTERNAL_APPS: RefCell<HashMap<String, Instant>> = RefCell::new(HashMap::new());
}

/// Held from the moment a request is taken up until it is answered, including the app lookup.
struct SitePromptTurn;

impl SitePromptTurn {
    fn take() -> Option<Self> {
        (!SITE_PROMPT_OPEN.replace(true)).then_some(Self)
    }
}

impl Drop for SitePromptTurn {
    fn drop(&mut self) {
        SITE_PROMPT_OPEN.set(false);
    }
}

pub(crate) fn register_browser_site_request_handler(cx: &App) {
    let async_cx = cx.to_async();
    crate::cef::set_browser_site_request_handler(Rc::new(move |request| {
        // CEF calls this from inside its message pump; the prompt opens on the app's next turn.
        async_cx
            .spawn(async move |cx| answer_browser_site_request(request, cx).await)
            .detach();
    }));
}

async fn answer_browser_site_request(request: BrowserSiteRequest, cx: &mut AsyncApp) {
    match request {
        BrowserSiteRequest::OpenExternalApp(request) => {
            answer_external_app_request(request, cx).await
        }
        BrowserSiteRequest::LocalNetworkAccess(request) => {
            answer_local_network_access_request(request, cx).await
        }
    }
}

/// CDXC:Browser 2026-10-03 WHY:
/// A link only another app opens (Okta Verify's `com-okta-authenticator:`, Zoom, Teams, `mailto:`)
/// asks first, naming the app, as Chrome does; a link no installed app handles is dropped, since
/// the OS would only show an error. Chrome's "always allow" checkbox is left out: GPUI prompts have
/// only buttons, and Ghostex has no site-settings page to take it back.
async fn answer_external_app_request(request: BrowserExternalAppRequest, cx: &mut AsyncApp) {
    let declined_key = format!("{}|{}", request.origin, request.scheme);
    let recently_declined = DECLINED_EXTERNAL_APPS.with(|declined| {
        declined
            .borrow()
            .get(&declined_key)
            .is_some_and(|at| at.elapsed() < DECLINED_EXTERNAL_APP_QUIET_PERIOD)
    });
    if recently_declined {
        return;
    }
    let Some(_turn) = SitePromptTurn::take() else {
        return;
    };
    let (url, scheme) = (request.url.clone(), request.scheme.clone());
    let Some(app_name) = cx
        .background_executor()
        .spawn(async move { registered_app_for_link(&url, &scheme) })
        .await
    else {
        return;
    };
    let site = site_label(&request.origin);
    let (message, detail, open_label) = match app_name {
        Some(app) => (
            format!("Open {app}?"),
            format!("{site} wants to open {app}."),
            format!("Open {app}"),
        ),
        None => (
            "Open this link in another app?".to_string(),
            format!("{site} wants to open a {}: link.", request.scheme),
            "Open".to_string(),
        ),
    };
    let choice = ask(&message, &detail, &[open_label.as_str(), "Cancel"], cx).await;
    if choice == Some(0) {
        cx.update(|cx| cx.open_url(&request.url));
    } else {
        DECLINED_EXTERNAL_APPS.with(|declined| {
            let mut declined = declined.borrow_mut();
            declined.retain(|_, at| at.elapsed() < DECLINED_EXTERNAL_APP_QUIET_PERIOD);
            declined.insert(declined_key, Instant::now());
        });
    }
}

/// CDXC:Browser 2026-10-03 WHY:
/// Chromium's Local Network Access asks before a public page reaches 127.0.0.1 or the local
/// network; sign-in pages use it to reach a desktop authenticator (Okta FastPass probes Okta
/// Verify's server on 127.0.0.1). Allow is stored by Chromium in the browser profile, so a site
/// asks once; Don't Allow only dismisses, so the site can ask again next time.
async fn answer_local_network_access_request(
    request: BrowserLocalNetworkAccessRequest,
    cx: &mut AsyncApp,
) {
    let Some(_turn) = SitePromptTurn::take() else {
        return;
    };
    let site = site_label(request.origin());
    let (message, detail) = if request.includes_local_network() {
        (
            format!("Allow {site} to connect to devices on your local network?"),
            "Only allow sites you trust.",
        )
    } else {
        (
            format!("Allow {site} to connect to apps on this computer?"),
            "Sign-in pages use this to reach an authenticator app such as Okta Verify. Only allow sites you trust.",
        )
    };
    if ask(&message, detail, &["Allow", "Don't Allow"], cx).await == Some(0) {
        request.allow();
    }
}

/// Asks in the window in front (an extension modal or panel is a window of its own). None when
/// there was no window or it closed before the user answered.
async fn ask(message: &str, detail: &str, answers: &[&str], cx: &mut AsyncApp) -> Option<usize> {
    let window: Option<AnyWindowHandle> = cx.update(|cx| {
        cx.active_window()
            .or_else(|| cx.windows().into_iter().next())
    });
    let answer = window?
        .update(cx, |_, window, cx| {
            window.prompt(PromptLevel::Info, message, Some(detail), answers, cx)
        })
        .ok()?;
    answer.await.ok()
}

/// `acme.okta.com` from `https://acme.okta.com` (Local Network Access hands over `https://linear.app/`).
fn site_label(origin: &str) -> String {
    match origin
        .split_once("://")
        .map(|(_, rest)| rest.trim_end_matches('/'))
    {
        Some(authority) if !authority.is_empty() => authority.to_string(),
        _ => "This page".to_string(),
    }
}

/// The app the OS opens `url` with: None when no app is registered for it, `Some(None)` when one is
/// but the OS does not name it.
#[cfg(target_os = "macos")]
fn registered_app_for_link(url: &str, _scheme: &str) -> Option<Option<String>> {
    crate::app::native_docs::open_externally::open_with_applications(url, true)
        .into_iter()
        .next()
        .map(|app| Some(app.name))
}

#[cfg(target_os = "windows")]
fn registered_app_for_link(_url: &str, scheme: &str) -> Option<Option<String>> {
    use windows_sys::Win32::System::Registry::{HKEY_CLASSES_ROOT, RRF_RT_ANY, RegGetValueW};
    use windows_sys::Win32::UI::Shell::{
        ASSOCF_INIT_IGNOREUNKNOWN, ASSOCF_IS_PROTOCOL, ASSOCSTR_FRIENDLYAPPNAME, AssocQueryStringW,
    };
    let wide = |text: &str| text.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let scheme = wide(scheme);
    let mut name = [0u16; 260];
    let mut len = name.len() as u32;
    // SAFETY: both strings are NUL-terminated and `len` is the buffer's length in characters.
    let named = unsafe {
        AssocQueryStringW(
            ASSOCF_IS_PROTOCOL | ASSOCF_INIT_IGNOREUNKNOWN,
            ASSOCSTR_FRIENDLYAPPNAME,
            scheme.as_ptr(),
            std::ptr::null(),
            name.as_mut_ptr(),
            &mut len,
        )
    } >= 0;
    if named {
        let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        let name = String::from_utf16_lossy(&name[..end]).trim().to_string();
        return Some((!name.is_empty()).then_some(name));
    }
    // A protocol registered under HKCR whose app Windows cannot name.
    let value = wide("URL Protocol");
    // SAFETY: NUL-terminated key and value names; no data is read back.
    let registered = unsafe {
        RegGetValueW(
            HKEY_CLASSES_ROOT,
            scheme.as_ptr(),
            value.as_ptr(),
            RRF_RT_ANY,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0;
    registered.then_some(None)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn registered_app_for_link(_url: &str, scheme: &str) -> Option<Option<String>> {
    let output = std::process::Command::new("xdg-mime")
        .args(["query", "default", &format!("x-scheme-handler/{scheme}")])
        .output();
    match output {
        Ok(output) if output.status.success() => {
            (!String::from_utf8_lossy(&output.stdout).trim().is_empty()).then_some(None)
        }
        // Without xdg-mime there is no way to tell; xdg-open decides.
        _ => Some(None),
    }
}
