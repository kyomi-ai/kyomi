use super::{SRC, extract_between};
use crate::pages::settings::datasources::{
    ModalOAuthStatusPanel, ModalOAuthStatusPanelProps, google_disconnect_blocked_by_sign_in,
};
use leptos::prelude::*;

fn panel_html(blocked: Option<bool>) -> String {
    let owner = Owner::new();
    let html = owner.with(|| {
        let (connected, _) = signal(true);
        let (email, _) = signal(Some("synthetic@example.com".to_string()));
        let (expired, _) = signal(false);
        let (connecting, set_connecting) = signal(false);
        let common = ModalOAuthStatusPanelProps::builder()
            .oauth_connected(connected)
            .oauth_email(email)
            .oauth_expired(expired)
            .oauth_connecting(connecting)
            .set_oauth_connecting(set_connecting)
            .provider_name("Google")
            .connect_url(Signal::stored(String::new()))
            .cfg_missing(Signal::stored(false))
            .on_disconnect(Callback::new(|()| {}))
            .disconnect_pending(Signal::stored(false))
            .on_recover(Callback::new(|_| {}));
        match blocked {
            Some(blocked) => {
                ModalOAuthStatusPanel(common.disconnect_blocked(Signal::stored(blocked)).build())
                    .to_html()
            }
            None => ModalOAuthStatusPanel(common.build()).to_html(),
        }
    });
    owner.cleanup();
    html
}

fn has_disabled_attribute(html: &str) -> bool {
    html.contains(" disabled ") || html.contains(" disabled>") || html.contains(" disabled=")
}

#[test]
fn global_google_only_disconnect_is_disabled_and_explained() {
    let html = panel_html(Some(true));
    assert!(has_disabled_attribute(&html), "{html}");
    assert!(
        html.contains("Google is your only sign-in method"),
        "{html}"
    );
    assert!(html.contains("href=\"/settings/security\""), "{html}");
    assert!(html.contains("Settings → Security"), "{html}");
}

#[test]
fn password_passkey_and_per_datasource_disconnects_remain_enabled() {
    for blocked in [Some(false), None] {
        let html = panel_html(blocked);
        assert!(!has_disabled_attribute(&html), "{html}");
        assert!(!html.contains("only sign-in method"), "{html}");
    }
}

#[test]
fn only_global_panel_receives_server_capability_and_unknown_stays_enabled() {
    let section = extract_between(
        SRC,
        "fn BigQueryAuthModeSection(",
        "fn SnowflakeAuthModeSection(",
    );
    assert!(section.contains(
        "Resource::new(|| (), |_| crate::server_fns::security::google_is_only_sign_in_method())"
    ));
    assert!(section.contains("google_disconnect_blocked_by_sign_in(google_only.try_get().flatten())"));
    assert_eq!(
        SRC.matches("disconnect_blocked=google_disconnect_blocked")
            .count(),
        1
    );
    let panel = extract_between(
        section,
        "<Show when=move || bq_auth_mode.get() == \"kyomi_oauth\">",
        "<Show when=move || bq_auth_mode.get() == \"enterprise_oauth\">",
    );
    assert!(panel.contains("disconnect_blocked=google_disconnect_blocked"));
    let shared = extract_between(SRC, "fn ModalOAuthStatusPanel(", "enum OAuthStatusSource");
    assert!(shared.contains("disconnect_pending.try_get().unwrap_or(false) || disconnect_blocked.try_get().unwrap_or(false)"));
    assert!(
        shared
            .contains("!disconnect_pending.get_untracked() && !disconnect_blocked.get_untracked()")
    );
}

#[test]
fn loading_and_failed_capability_lookup_do_not_block_disconnect() {
    assert!(!google_disconnect_blocked_by_sign_in(None));
    assert!(!google_disconnect_blocked_by_sign_in(Some(Err(
        ServerFnError::new("lookup failed")
    ))));
    assert!(!google_disconnect_blocked_by_sign_in(Some(Ok(false))));
    assert!(google_disconnect_blocked_by_sign_in(Some(Ok(true))));
}
