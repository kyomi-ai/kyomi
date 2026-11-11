// SPDX-License-Identifier: AGPL-3.0-or-later
use super::*;
#[test]
fn personal_mcp_oauth_challenge_and_return_contracts() {
    let challenge=parse_bearer_challenge("Basic realm=\"legacy\", Bearer realm=\"a,b\", resource_metadata=\"https://resource.example.com/meta\", scope=\"read write\"").unwrap();
    assert_eq!(challenge.get("realm").unwrap(), "a,b");
    assert_eq!(challenge.get("scope").unwrap(), "read write");
    assert!(parse_bearer_challenge("Bearer scope=\"read\", scope=\"admin\"").is_err());
    assert!(parse_bearer_challenge("Bearer scope=\"unfinished").is_err());
    assert!(parse_bearer_challenge("☃☃☃").is_ok());
    assert_eq!(
        authorization_required(401, None).unwrap(),
        Some(McpAuthorizationRequired::Reconnect)
    );
    assert_eq!(
        authorization_required(
            403,
            Some("Bearer error=\"insufficient_scope\", scope=\"write\"")
        )
        .unwrap(),
        Some(McpAuthorizationRequired::StepUp {
            scopes: vec!["write".into()]
        })
    );
    for path in [
        "//evil.example.com",
        "https://evil.example.com",
        "/\\evil",
        "/%2f%2fevil",
        "/ok\r\nLocation:evil",
    ] {
        assert!(safe_return_path(path).is_err());
    }
    assert_eq!(
        safe_return_path("/settings/profile?tab=mcp").unwrap(),
        "/settings/profile?tab=mcp"
    );
    assert_eq!(
        authorization_metadata_urls(ISSUER).unwrap(),
        vec![
            "https://accounts.example.com/.well-known/oauth-authorization-server/tenant",
            "https://accounts.example.com/.well-known/openid-configuration/tenant",
            "https://accounts.example.com/tenant/.well-known/openid-configuration"
        ]
    );
    assert!(https_url("http://private.example.com").is_err());
    let mut credential_url = Url::parse("https://provider.example.com").unwrap();
    credential_url.set_username("name").unwrap();
    credential_url
        .set_password(Some("test-url-secret"))
        .unwrap();
    assert!(https_url(credential_url.as_str()).is_err());
}
#[tokio::test]
async fn personal_mcp_oauth_registration_selection_and_unsafe_egress() {
    let provider = Arc::new(Provider::default());
    let mut service = service(provider.clone());
    let mut metadata = AuthorizationMetadata {
        issuer: ISSUER.into(),
        authorization_endpoint: "https://accounts.example.com/authorize".into(),
        token_endpoint: "https://accounts.example.com/token".into(),
        registration_endpoint: None,
        response_types_supported: vec!["code".into()],
        code_challenge_methods_supported: vec!["S256".into()],
        token_endpoint_auth_methods_supported: Some(vec!["none".into()]),
        client_id_metadata_document_supported: true,
        authorization_response_iss_parameter_supported: false,
    };
    assert!(service.client(&metadata).await.is_err());
    service.settings.client_metadata_url =
        Some("https://kyomi.example.com/api/v1/personal-mcp/oauth/client-metadata.json".into());
    assert_eq!(
        service.registration_kind(&metadata).unwrap(),
        "client_metadata_document"
    );
    assert!(
        service
            .client(&metadata)
            .await
            .unwrap()
            .client_secret
            .is_none()
    );
    service.settings.registered_clients.insert(
        ISSUER.into(),
        RegisteredClient {
            client_id: "test-pre-registered".into(),
            client_secret: None,
            token_endpoint_auth_method: "none".into(),
        },
    );
    assert_eq!(
        service.registration_kind(&metadata).unwrap(),
        "pre_registered"
    );
    assert_eq!(
        service.client(&metadata).await.unwrap().client_id,
        "test-pre-registered"
    );
    metadata.issuer = "https://different.example.com".into();
    metadata.client_id_metadata_document_supported = false;
    assert!(service.client(&metadata).await.is_err());
    assert!(provider.requests.lock().unwrap().is_empty());
    // Real safe adapter rejects the actual private destination before a socket.
    let service =
        OAuthService::production(OAuthSettings::new("https://kyomi.example.com").unwrap()).unwrap();
    let private = format!("https://{}/metadata", std::net::Ipv4Addr::LOCALHOST);
    assert!(
        service
            .document::<serde_json::Value>(&private)
            .await
            .is_err()
    );
}
