//! Kovion's own iOS sideloader.
//!
//! Signs and installs an .ipa onto a connected iPhone using a free Apple ID, the same way
//! AltServer/SideStore/iloader do, built directly on the open-source `isideload` library
//! (MIT, by nab138) so Kovion iTools never has to open another app to do it.
//!
//! Protocol: one JSON request line on stdin, then newline-delimited JSON "event" lines on
//! stdout as it works. Two events ask the caller to write a reply line back on stdin:
//! "need_2fa" (reply with a `TwoFactorCallbackResponse`, e.g. `{"SubmitCode":"123456"}`)
//! and nothing else blocks — team and certificate limits are both handled automatically.

use std::{io::Write, path::PathBuf};

use idevice::usbmuxd::{UsbmuxdAddr, UsbmuxdConnection};
use isideload::{
    anisette::remote_v3::RemoteV3AnisetteProvider,
    auth::apple_account::{AppleAccount, TwoFactorCallbackParams, TwoFactorCallbackResponse},
    dev::{certificates::DevelopmentCertificate, developer_session::DeveloperSession},
    sideload::{builder::MaxCertsBehavior, SideloaderBuilder, TeamSelection},
    util::keyring_storage::KeyringStorage,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;

#[derive(Deserialize)]
struct Request {
    apple_id: String,
    password: String,
    ipa_path: String,
    /// Keyring service name, scoped so this never touches another tool's saved data.
    #[serde(default = "default_service")]
    service: String,
}

fn default_service() -> String {
    "Kovion".to_string()
}

fn emit(v: serde_json::Value) {
    println!("{v}");
    let _ = std::io::stdout().flush();
}

fn status(msg: impl Into<String>) {
    emit(json!({"event": "status", "message": msg.into()}));
}

fn fail(msg: impl Into<String>) -> ! {
    emit(json!({"event": "error", "message": msg.into()}));
    std::process::exit(1);
}

/// `MaxCertsBehavior::Revoke` never actually calls a callback (it revokes the oldest certs
/// itself), but `SideloaderBuilder` still needs a concrete callback type to name - a bare fn
/// pointer satisfies the bound without ever being called.
type MaxCertsCb =
    fn(Vec<DevelopmentCertificate>) -> std::future::Ready<Result<Option<Vec<String>>, rootcause::Report>>;

#[tokio::main]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");
    if let Err(e) = isideload::init() {
        fail(format!("Could not start: {e}"));
    }
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::WARN)
        .init();

    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
        fail("No request given");
    }
    let req: Request = match serde_json::from_str(line.trim()) {
        Ok(r) => r,
        Err(e) => fail(format!("Bad request: {e}")),
    };

    status("Checking for your iPhone");
    let usbmuxd = match UsbmuxdConnection::default().await {
        Ok(c) => c,
        Err(e) => fail(format!("Couldn't reach usbmuxd: {e}")),
    };
    let mut usbmuxd = usbmuxd;
    let devices = match usbmuxd.get_devices().await {
        Ok(d) => d,
        Err(e) => fail(format!("Couldn't list devices: {e}")),
    };
    let Some(device) = devices.first() else {
        fail("No iPhone connected. Plug it in and unlock it.");
    };
    let provider = device.to_provider(UsbmuxdAddr::from_env_var().unwrap(), "kovion-sideload");

    status("Signing in to your Apple Account");
    let reader = Arc::new(Mutex::new(BufReader::new(tokio::io::stdin()).lines()));

    let two_factor_callback = move |params: TwoFactorCallbackParams| {
        let reader = reader.clone();
        async move {
            emit(json!({
                "event": "need_2fa",
                "unknown": params.unknown,
                "sms": params.sms,
                "numbers": params.numbers,
                "lastError": params.last_error,
            }));
            let mut reader = reader.lock().await;
            let reply = loop {
                match reader.next_line().await {
                    Ok(Some(l)) if !l.trim().is_empty() => break l,
                    Ok(Some(_)) => continue,
                    _ => fail("No reply for the verification code"),
                }
            };
            let resp: TwoFactorCallbackResponse = serde_json::from_str(reply.trim())
                .unwrap_or(TwoFactorCallbackResponse::Abort);
            Ok(resp)
        }
    };

    let account = AppleAccount::builder(&req.apple_id)
        .anisette_provider(RemoteV3AnisetteProvider::default().unwrap().set_serial_number("0".to_string()))
        .login(&req.password, two_factor_callback)
        .await;

    let mut account = match account {
        Ok(a) => a,
        Err(e) => fail(format!("Apple sign-in failed: {e}")),
    };

    status("Preparing your developer account");
    let dev_session = match DeveloperSession::from_account(&mut account).await {
        Ok(s) => s,
        Err(e) => fail(format!("Couldn't start a developer session: {e}")),
    };

    let mut sideloader = SideloaderBuilder::<MaxCertsCb>::new(dev_session, req.apple_id.clone())
        .team_selection(TeamSelection::First)
        .max_certs_behavior(MaxCertsBehavior::Revoke)
        .storage(Box::new(KeyringStorage::new(req.service.clone())))
        .machine_name("Kovion iTools".to_string())
        .build();

    status("Signing and installing Kovion");
    let app_path = PathBuf::from(&req.ipa_path);
    let progress = |fraction: f32| async move {
        emit(json!({"event": "progress", "fraction": fraction}));
    };
    let result = sideloader
        .install_app(&provider, app_path, true, Some(progress), None)
        .await;

    match result {
        Ok(_) => emit(json!({"event": "done", "ok": true})),
        Err(e) => fail(format!("{e}")),
    }
}
