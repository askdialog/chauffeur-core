//! The Office's proximity calls in WebKitGTK: the main window's microphone, camera and WebRTC.
//!
//! WebKitGTK refuses `getUserMedia` when the app does not answer its `permission-request`, and
//! leaves `RTCPeerConnection` out unless `enable-webrtc` is on (and the library was built with
//! WebRTC). macOS needs none of this: WKWebView asks the system, which reads Info.plist.

use tauri::Manager;
use webkit2gtk::glib::prelude::Cast;
use webkit2gtk::{
    PermissionRequestExt, SettingsExt, UserMediaPermissionRequest, UserMediaPermissionRequestExt,
    WebViewExt,
};

/// Turns media streams and WebRTC on in the main window and allows its requests for the
/// microphone and the camera. Every other request (screen capture, geolocation, notifications…)
/// keeps WebKitGTK's default answer.
pub fn enable(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let result = window.with_webview(|webview| {
        let webview = webview.inner();
        if let Some(settings) = webview.settings() {
            settings.set_enable_media_stream(true);
            settings.set_enable_webrtc(true);
        }
        webview.connect_permission_request(|_, request| {
            let Some(media) = request.downcast_ref::<UserMediaPermissionRequest>() else {
                return false;
            };
            // The user's consent is the Office's own microphone and camera switches, off until
            // turned on and remembered on this computer: the page asks only for what they allow,
            // so there is no second prompt here.
            if !(media.is_for_audio_device() || media.is_for_video_device()) {
                return false;
            }
            request.allow();
            true
        });
    });
    if let Err(error) = result {
        eprintln!("chauffeur: could not allow the microphone and camera in the window: {error}");
    }
}
