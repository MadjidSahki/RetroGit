//! macOS notifications through `UNUserNotificationCenter` (inside `RetroGit.app` only): they
//! carry the app's name and icon, and a click is reported back to the app.
//!
//! The only module with `unsafe`: calls into Apple's frameworks, each one commented.
#![allow(unsafe_code)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AnyThread, define_class, msg_send};
use objc2_foundation::{NSBundle, NSDictionary, NSError, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
    UNNotificationSound, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};

/// Key of the link in a notification's user info.
const LINK_KEY: &str = "link";

static NATIVE: AtomicBool = AtomicBool::new(false);
static CLICKED: Mutex<Vec<String>> = Mutex::new(Vec::new());
static WAKE: Mutex<Option<Box<dyn Fn() + Send>>> = Mutex::new(None);

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no ivars.
    #[unsafe(super(NSObject))]
    #[name = "RetroGitNotificationDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        /// Show notifications while RetroGit is in front too.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            handler.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            handler: &block2::DynBlock<dyn Fn()>,
        ) {
            let info = response.notification().request().content().userInfo();
            let key = NSString::from_str(LINK_KEY);
            let link = info
                .objectForKey(&key)
                .and_then(|o| o.downcast::<NSString>().ok())
                .map(|s| s.to_string());
            log::info!("notification clicked: {link:?}");
            if let Some(link) = link
                && let Ok(mut q) = CLICKED.lock()
            {
                q.push(link);
            }
            if let Ok(w) = WAKE.lock()
                && let Some(w) = w.as_ref()
            {
                w();
            }
            handler.call(());
        }
    }
);

/// Running from an app bundle (`UNUserNotificationCenter` needs one).
fn in_bundle() -> bool {
    NSBundle::mainBundle().bundleIdentifier().is_some()
}

/// Call first thing in `main` (before the window): the delegate must be in place to receive
/// the click that launched the app. Does nothing outside `RetroGit.app`.
pub fn init() {
    if !in_bundle() {
        return;
    }
    let center = UNUserNotificationCenter::currentNotificationCenter();
    // SAFETY: plain NSObject subclass without ivars: `init` is NSObject's.
    let delegate: Retained<Delegate> = unsafe { msg_send![Delegate::alloc(), init] };
    center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    // The center keeps a weak reference: the delegate lives as long as the app.
    std::mem::forget(delegate);
    let done = RcBlock::new(|granted: Bool, _err: *mut NSError| {
        log::info!("notification permission granted: {}", granted.as_bool());
    });
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &done,
    );
    NATIVE.store(true, Ordering::SeqCst);
}

/// Native notifications are in use (inside `RetroGit.app`).
pub fn native() -> bool {
    NATIVE.load(Ordering::SeqCst)
}

/// `wake` is called (from any thread) after a click, to repaint the window.
pub fn set_waker(wake: impl Fn() + Send + 'static) {
    if let Ok(mut w) = WAKE.lock() {
        *w = Some(Box::new(wake));
    }
}

/// Links of the notifications clicked since the last call.
pub fn take_clicked() -> Vec<String> {
    CLICKED
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

pub fn send(title: &str, body: &str, link: Option<&str>) -> Result<(), String> {
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    content.setSound(Some(&UNNotificationSound::defaultSound()));
    if let Some(link) = link {
        let key = NSString::from_str(LINK_KEY);
        let value = NSString::from_str(link);
        let info: Retained<NSDictionary<NSString, AnyObject>> =
            NSDictionary::from_slices(&[&*key], &[&*value as &AnyObject]);
        // SAFETY: a property-list dictionary (string keys and values).
        let info: Retained<NSDictionary> = unsafe { Retained::cast_unchecked(info) };
        // SAFETY: the dictionary holds only NSString keys and values (a property list).
        unsafe { content.setUserInfo(&info) };
    }
    let id = NSString::from_str(&format!(
        "retrogit-{}",
        std::process::id() as u64 ^ now_nanos()
    ));
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&id, &content, None);
    UNUserNotificationCenter::currentNotificationCenter()
        .addNotificationRequest_withCompletionHandler(&request, None);
    Ok(())
}

fn now_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}
