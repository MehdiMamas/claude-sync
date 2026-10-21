//! macOS desktop notifications through `UNUserNotificationCenter`.
//!
//! The plugin's macOS path (`NSUserNotificationCenter`) never asks for permission, so
//! macOS never lists the app under System Settings > Notifications and nothing shows.
//! This asks once at startup, sends through the modern API, and opens the Notifications
//! screen when a notification is clicked.
use crate::{notify, window};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{Bool, ProtocolObject};
use objc2::{define_class, msg_send, AnyThread};
use objc2_foundation::{NSBundle, NSError, NSObject, NSObjectProtocol, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
    UNNotificationSound, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};
use std::ptr::NonNull;
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::Duration;
use tauri::AppHandle;

static APP: OnceLock<AppHandle> = OnceLock::new();

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "ClaudeSyncNotificationDelegate"]
    #[ivars = ()]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        // Show the banner even while the app is frontmost.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            done: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            done.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            _response: &UNNotificationResponse,
            done: &block2::DynBlock<dyn Fn()>,
        ) {
            if let Some(app) = APP.get() {
                let a = app.clone();
                let _ = app.run_on_main_thread(move || window::open(&a, Some(notify::ROUTE)));
            }
            done.call(());
        }
    }
);

/// `UNUserNotificationCenter` throws when the process is not an app bundle (`cargo run`).
fn in_bundle() -> bool {
    NSBundle::mainBundle().bundleIdentifier().is_some()
}

/// Register the delegate and ask for permission (macOS shows its prompt the first time).
pub fn init(app: &AppHandle) {
    if !in_bundle() {
        tracing::info!("not running from an app bundle: no macOS notifications");
        return;
    }
    let _ = APP.set(app.clone());
    let center = UNUserNotificationCenter::currentNotificationCenter();
    let delegate: Retained<Delegate> =
        unsafe { msg_send![super(Delegate::alloc().set_ivars(())), init] };
    center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    // The center keeps only a weak reference.
    std::mem::forget(delegate);
    let done = RcBlock::new(|granted: Bool, err: *mut NSError| {
        if let Some(e) = unsafe { err.as_ref() } {
            // macOS answers this way, without a prompt, once the app was denied (or
            // dismissed) before: the choice sticks to the bundle id until the user flips
            // it in System Settings > Notifications.
            tracing::warn!(
                "notification permission request failed: {e}. If notifications were denied \
                 earlier, turn them on in System Settings > Notifications > claude-sync"
            );
        } else {
            tracing::info!(granted = granted.as_bool(), "notification permission");
        }
    });
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert
            | UNAuthorizationOptions::Sound
            | UNAuthorizationOptions::Badge,
        &done,
    );
}

/// Send one notification. Err when macOS refused it or the app is not bundled.
pub fn show(title: &str, body: &str) -> Result<(), String> {
    if !in_bundle() {
        return Err("not running from an app bundle".into());
    }
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    content.setSound(Some(&UNNotificationSound::defaultSound()));
    let id = NSString::from_str(&uuid_like());
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&id, &content, None);
    let done = RcBlock::new(|err: *mut NSError| {
        if let Some(e) = unsafe { err.as_ref() } {
            tracing::warn!("notification failed: {e}");
        }
    });
    UNUserNotificationCenter::currentNotificationCenter()
        .addNotificationRequest_withCompletionHandler(&request, Some(&done));
    Ok(())
}

/// `granted`, `denied`, `not_asked`, or `unknown`.
pub fn permission() -> &'static str {
    if !in_bundle() {
        return "unknown";
    }
    let (tx, rx) = mpsc::channel();
    let done = RcBlock::new(
        move |settings: NonNull<objc2_user_notifications::UNNotificationSettings>| {
            let status = unsafe { settings.as_ref() }.authorizationStatus();
            let _ = tx.send(status);
        },
    );
    UNUserNotificationCenter::currentNotificationCenter()
        .getNotificationSettingsWithCompletionHandler(&done);
    match rx.recv_timeout(Duration::from_secs(2)) {
        Ok(UNAuthorizationStatus::Denied) => "denied",
        Ok(UNAuthorizationStatus::NotDetermined) => "not_asked",
        Ok(_) => "granted",
        Err(_) => "unknown",
    }
}

fn uuid_like() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("claude-sync-{t}-{}", N.fetch_add(1, Ordering::Relaxed))
}
