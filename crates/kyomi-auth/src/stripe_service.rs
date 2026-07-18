// SPDX-License-Identifier: AGPL-3.0-or-later

//! Stripe service — wraps the `async-stripe` crate for all Stripe API
//! interactions: customers, checkout sessions, subscriptions, webhooks,
//! invoices, and billing portal sessions.
//!
//! This is the Rust equivalent of Python's `stripe_service.py`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use stripe::{Client, StripeError};
use stripe_billing::{
    invoice::ListInvoice,
    subscription::{
        CancelSubscription, CreateSubscription, CreateSubscriptionItems, RetrieveSubscription,
        UpdateSubscription,
    },
    subscription_item::{DeleteSubscriptionItem, UpdateSubscriptionItem},
};
use stripe_checkout::checkout_session::{
    CreateCheckoutSession, CreateCheckoutSessionAutomaticTax, CreateCheckoutSessionCustomText,
    CreateCheckoutSessionCustomerUpdate, CreateCheckoutSessionLineItems,
    CreateCheckoutSessionPaymentMethodTypes, CreateCheckoutSessionSubscriptionData,
    CreateCheckoutSessionTaxIdCollection, CreateCheckoutSessionCustomerUpdateName,
    CreateCheckoutSessionCustomerUpdateAddress, CustomTextPositionParam,
    RetrieveCheckoutSession,
};
use stripe_checkout::CheckoutSessionMode;
use stripe_core::customer::CreateCustomer;
use stripe_shared::{
    Subscription, SubscriptionStatus,
};
use stripe_types::Expandable;
use stripe_webhook::Webhook;

// ─── Public types ───────────────────────────────────────────────────────────

/// Parameters for creating a Stripe Checkout session.
#[derive(Debug)]
pub struct CheckoutParams {
    pub customer_id: String,
    pub price_id: String,
    pub success_url: String,
    pub cancel_url: String,
    pub workspace_id: String,
    /// Number of users (subscription quantity).
    pub quantity: u64,
    pub trial_days: u32,
}

/// Result of creating a checkout session.
#[derive(Debug, Serialize, Deserialize)]
pub struct CheckoutResult {
    pub checkout_url: String,
    pub session_id: String,
}

/// Parsed subscription data from a Stripe subscription object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionData {
    pub tier: String,
    pub status: String,
    pub billing_cycle: Option<String>,
    pub user_limit: i32,
    pub period_start: Option<DateTime<Utc>>,
    pub period_end: Option<DateTime<Utc>>,
    pub stripe_subscription_id: String,
    pub stripe_customer_id: String,
}

/// A simplified invoice record for API responses.
#[derive(Debug, Serialize, Deserialize)]
pub struct InvoiceData {
    pub invoice_id: String,
    pub amount_paid: i64,
    pub currency: String,
    pub status: Option<String>,
    pub hosted_invoice_url: Option<String>,
    pub invoice_pdf: Option<String>,
    pub created: Option<i64>,
}

/// Parameters for creating an embedded checkout session (subscription).
#[derive(Debug)]
pub struct EmbeddedCheckoutParams {
    pub customer_id: String,
    pub price_id: String,
    pub workspace_id: String,
    pub quantity: u64,
    pub trial_days: u32,
}

/// Parameters for creating an embedded payment checkout session (bundle purchases).
#[derive(Debug)]
pub struct EmbeddedPaymentCheckoutParams {
    pub customer_id: String,
    pub price_id: String,
    pub workspace_id: String,
    pub purchase_type: String,
    pub quantity: u64,
}

/// Result of creating an embedded checkout session.
#[derive(Debug, Serialize, Deserialize)]
pub struct EmbeddedCheckoutResult {
    pub client_secret: String,
    pub session_id: String,
}

/// Status of a checkout session (for verifying completion).
#[derive(Debug, Serialize, Deserialize)]
pub struct CheckoutSessionStatus {
    pub status: String,
    pub payment_status: String,
}

/// Parameters for creating a one-time payment checkout session (e.g. bundle purchases).
#[derive(Debug)]
pub struct PaymentCheckoutParams {
    pub customer_id: String,
    pub price_id: String,
    pub success_url: String,
    pub cancel_url: String,
    pub workspace_id: String,
    /// Type of purchase (e.g. "ai_bundle", "analytics_bundle").
    pub purchase_type: String,
    /// Number of bundles to purchase (minimum 1).
    pub quantity: u64,
}

// ─── Retry classification ───────────────────────────────────────────────────

/// Returns `true` if the Stripe error is transient and the operation may succeed
/// on retry.
///
/// Retryable conditions:
/// - `Timeout` — network timeout talking to Stripe.
/// - `ClientError` — network-level error (connection reset, DNS failure, etc.).
/// - `Stripe(_, status)` where status is 429 (rate limit), 502, 503, or 504
///   (gateway/upstream errors).
///
/// Permanent errors that must not be retried:
/// - `Stripe(_, 400..=404)` — bad request, auth failure, not found.
/// - `JSONDeserialize` — response parsing error (not a transient condition).
/// - `ConfigError` — client misconfiguration.
fn is_stripe_transient(e: &StripeError) -> bool {
    match e {
        StripeError::Timeout => true,
        StripeError::ClientError(_) => true,
        StripeError::Stripe(_, status) => {
            kyomi_core::retry::is_transient_http_status(*status)
        }
        StripeError::JSONDeserialize(_) | StripeError::ConfigError(_) => false,
    }
}

// ─── Service ────────────────────────────────────────────────────────────────

/// Wraps the `async-stripe` `Client` and provides typed methods for
/// all Stripe operations used by Kyomi's billing system.
pub struct StripeService {
    client: Client,
    webhook_secret: String,
}

impl StripeService {
    /// Create a new `StripeService` from a Stripe secret key and webhook secret.
    pub fn new(secret_key: &str, webhook_secret: &str) -> Self {
        let client = Client::new(secret_key);
        Self {
            client,
            webhook_secret: webhook_secret.to_string(),
        }
    }

    /// Returns a reference to the underlying Stripe API client.
    ///
    /// Used by callers that need to make Stripe API calls not covered by
    /// the typed methods on this service (e.g. `CreateSubscriptionItem`).
    pub fn client(&self) -> &Client {
        &self.client
    }

    // ── Customer ─────────────────────────────────────────────────────────

    /// Create a Stripe customer for a workspace.
    ///
    /// Returns the Stripe customer ID.
    pub async fn create_customer(
        &self,
        email: &str,
        workspace_id: &str,
        workspace_name: &str,
    ) -> Result<String, StripeError> {
        let description = format!("Kyomi Workspace: {workspace_name}");
        let metadata: std::collections::HashMap<String, String> = [
            ("workspace_id".to_string(), workspace_id.to_string()),
            ("workspace_name".to_string(), workspace_name.to_string()),
            ("app".to_string(), "kyomi".to_string()),
        ]
        .into_iter()
        .collect();

        let customer = CreateCustomer::new()
            .email(email)
            .description(&description)
            .metadata(metadata)
            .send(&self.client)
            .await?;

        tracing::info!(
            customer_id = %customer.id,
            workspace_id,
            "Created Stripe customer"
        );

        Ok(customer.id.to_string())
    }

    // ── Subscription (direct creation) ─────────────────────────────────

    /// Create a Stripe subscription for a workspace.
    ///
    /// Used at signup to create a Cloud subscription with a 30-day trial.
    /// No payment method required during trial — Stripe allows this.
    pub async fn create_subscription(
        &self,
        customer_id: &str,
        price_id: &str,
        quantity: u64,
        trial_days: u32,
        workspace_id: &str,
    ) -> Result<SubscriptionData, StripeError> {
        let metadata: std::collections::HashMap<String, String> = [
            ("workspace_id".to_string(), workspace_id.to_string()),
            ("brand".to_string(), "kyomi".to_string()),
            ("app".to_string(), "kyomi".to_string()),
        ]
        .into_iter()
        .collect();

        let subscription = CreateSubscription::new()
            .customer(customer_id)
            .items(vec![CreateSubscriptionItems {
                price: Some(price_id.to_string()),
                quantity: Some(quantity),
                ..Default::default()
            }])
            .trial_period_days(trial_days)
            .metadata(metadata)
            .description("Kyomi Cloud")
            .send(&self.client)
            .await?;

        tracing::info!(
            subscription_id = %subscription.id,
            customer_id,
            workspace_id,
            trial_days,
            "Created Stripe subscription"
        );

        self.parse_subscription_data(&subscription).await
    }

    // ── Checkout ─────────────────────────────────────────────────────────

    /// Create a Stripe Checkout session for a new subscription.
    pub async fn create_checkout_session(
        &self,
        params: &CheckoutParams,
    ) -> Result<CheckoutResult, StripeError> {
        // Single line item — quantity is the number of users
        let line_items = vec![CreateCheckoutSessionLineItems {
            price: Some(params.price_id.clone()),
            quantity: Some(params.quantity),
            ..Default::default()
        }];

        // Subscription metadata — workspace_id, brand, and app identifier
        let sub_metadata: std::collections::HashMap<String, String> = [
            ("workspace_id".to_string(), params.workspace_id.clone()),
            ("brand".to_string(), "kyomi".to_string()),
            ("app".to_string(), "kyomi".to_string()),
        ]
        .into_iter()
        .collect();

        let mut sub_data = CreateCheckoutSessionSubscriptionData {
            description: Some("Kyomi Cloud".to_string()),
            metadata: Some(sub_metadata),
            ..Default::default()
        };

        if params.trial_days > 0 {
            sub_data.trial_period_days = Some(params.trial_days);
        }

        // Session metadata
        let session_metadata: std::collections::HashMap<String, String> = [
            ("workspace_id".to_string(), params.workspace_id.clone()),
            ("brand".to_string(), "kyomi".to_string()),
        ]
        .into_iter()
        .collect();

        let session = CreateCheckoutSession::new()
            .customer(&params.customer_id)
            .mode(CheckoutSessionMode::Subscription)
            .payment_method_types(vec![CreateCheckoutSessionPaymentMethodTypes::Card])
            .line_items(line_items)
            .success_url(&params.success_url)
            .cancel_url(&params.cancel_url)
            .subscription_data(sub_data)
            .metadata(session_metadata)
            .custom_text(CreateCheckoutSessionCustomText {
                submit: Some(CustomTextPositionParam::new("Subscribe to Kyomi Cloud")),
                ..Default::default()
            })
            .automatic_tax(CreateCheckoutSessionAutomaticTax {
                enabled: true,
                liability: None,
            })
            .tax_id_collection(CreateCheckoutSessionTaxIdCollection::new(true))
            .customer_update(CreateCheckoutSessionCustomerUpdate {
                name: Some(CreateCheckoutSessionCustomerUpdateName::Auto),
                address: Some(CreateCheckoutSessionCustomerUpdateAddress::Auto),
                ..Default::default()
            })
            .send(&self.client)
            .await?;

        tracing::info!(
            session_id = %session.id,
            workspace_id = %params.workspace_id,
            "Created checkout session"
        );

        let checkout_url = session.url.ok_or_else(|| {
            StripeError::ClientError(
                "Checkout session created but no URL returned".into(),
            )
        })?;

        Ok(CheckoutResult {
            session_id: session.id.to_string(),
            checkout_url,
        })
    }

    /// Create a one-time payment checkout session (for bundle purchases).
    pub async fn create_payment_checkout_session(
        &self,
        params: &PaymentCheckoutParams,
    ) -> Result<CheckoutResult, StripeError> {
        let line_items = vec![CreateCheckoutSessionLineItems {
            price: Some(params.price_id.clone()),
            quantity: Some(params.quantity),
            ..Default::default()
        }];

        let session_metadata: std::collections::HashMap<String, String> = [
            ("workspace_id".to_string(), params.workspace_id.clone()),
            ("brand".to_string(), "kyomi".to_string()),
            ("purchase_type".to_string(), params.purchase_type.clone()),
            ("bundle_quantity".to_string(), params.quantity.to_string()),
        ]
        .into_iter()
        .collect();

        let session = CreateCheckoutSession::new()
            .customer(&params.customer_id)
            .mode(CheckoutSessionMode::Payment)
            .payment_method_types(vec![CreateCheckoutSessionPaymentMethodTypes::Card])
            .line_items(line_items)
            .success_url(&params.success_url)
            .cancel_url(&params.cancel_url)
            .metadata(session_metadata)
            .automatic_tax(CreateCheckoutSessionAutomaticTax {
                enabled: true,
                liability: None,
            })
            .customer_update(CreateCheckoutSessionCustomerUpdate {
                address: Some(CreateCheckoutSessionCustomerUpdateAddress::Auto),
                ..Default::default()
            })
            .send(&self.client)
            .await?;

        tracing::info!(
            session_id = %session.id,
            workspace_id = %params.workspace_id,
            purchase_type = %params.purchase_type,
            "Created payment checkout session"
        );

        let checkout_url = session.url.ok_or_else(|| {
            StripeError::ClientError(
                "Checkout session created but no URL returned".into(),
            )
        })?;

        Ok(CheckoutResult {
            session_id: session.id.to_string(),
            checkout_url,
        })
    }

    // ── Subscription management ──────────────────────────────────────────

    /// Update an existing subscription to a new tier/billing cycle.
    ///
    /// Replaces the subscription's main price item with `new_price_id`,
    /// preserving any additional-users line items.
    pub async fn update_subscription(
        &self,
        subscription_id: &str,
        new_price_id: &str,
        tier: &str,
        billing_cycle: &str,
    ) -> Result<SubscriptionData, StripeError> {
        // Retrieve current subscription to get item ID.
        let subscription = kyomi_core::retry::retry_with_backoff_classified(
            || async { RetrieveSubscription::new(subscription_id).send(&self.client).await },
            is_stripe_transient,
        )
        .await?;

        let items = &subscription.items.data;
        if items.is_empty() {
            return Err(StripeError::ClientError(
                "Subscription has no items".into(),
            ));
        }

        let first_item_id = items[0].id.to_string();

        let mut update_metadata = subscription.metadata.clone();
        update_metadata.insert("tier".to_string(), tier.to_string());
        update_metadata.insert("billing_cycle".to_string(), billing_cycle.to_string());

        // Build update parameters using the builder.
        let updated = kyomi_core::retry::retry_with_backoff_classified(
            || {
                let first_item_id = first_item_id.clone();
                let update_metadata = update_metadata.clone();
                async move {
                    UpdateSubscription::new(subscription_id)
                        .items(vec![stripe_billing::subscription::UpdateSubscriptionItems {
                            id: Some(first_item_id),
                            price: Some(new_price_id.to_string()),
                            ..Default::default()
                        }])
                        .metadata(update_metadata)
                        .proration_behavior(
                            stripe_billing::subscription::UpdateSubscriptionProrationBehavior::CreateProrations,
                        )
                        .send(&self.client)
                        .await
                }
            },
            is_stripe_transient,
        )
        .await?;

        tracing::info!(
            subscription_id,
            tier,
            billing_cycle,
            "Updated subscription"
        );

        self.parse_subscription_data(&updated).await
    }

    /// Add a new subscription item (e.g. additional users) to an existing subscription.
    ///
    /// Returns the new subscription item's ID.
    pub async fn add_subscription_item(
        &self,
        subscription_id: &str,
        price_id: &str,
        quantity: u64,
    ) -> Result<String, StripeError> {
        let new_item = kyomi_core::retry::retry_with_backoff_classified(
            || async {
                stripe_billing::subscription_item::CreateSubscriptionItem::new(subscription_id)
                    .price(price_id)
                    .quantity(quantity)
                    .send(&self.client)
                    .await
            },
            is_stripe_transient,
        )
        .await?;

        tracing::info!(
            subscription_id,
            quantity,
            item_id = %new_item.id,
            "Added subscription item"
        );

        Ok(new_item.id.to_string())
    }

    /// Update the quantity of additional users on a Team subscription.
    pub async fn update_subscription_quantity(
        &self,
        subscription_id: &str,
        item_id: &str,
        new_quantity: i64,
    ) -> Result<(), StripeError> {
        if new_quantity == 0 {
            // Remove the additional users line item.
            kyomi_core::retry::retry_with_backoff_classified(
                || async { DeleteSubscriptionItem::new(item_id).send(&self.client).await },
                is_stripe_transient,
            )
            .await?;
            tracing::info!(
                subscription_id,
                "Removed additional users from subscription"
            );
        } else {
            kyomi_core::retry::retry_with_backoff_classified(
                || async {
                    UpdateSubscriptionItem::new(item_id)
                        .quantity(new_quantity as u64)
                        .send(&self.client)
                        .await
                },
                is_stripe_transient,
            )
            .await?;
            tracing::info!(
                subscription_id,
                new_quantity,
                "Updated additional users quantity"
            );
        }

        Ok(())
    }

    /// Get the current seat count (quantity on the first subscription item).
    pub async fn get_subscription_quantity(
        &self,
        subscription_id: &str,
    ) -> Result<u64, StripeError> {
        let subscription = kyomi_core::retry::retry_with_backoff_classified(
            || async { RetrieveSubscription::new(subscription_id).send(&self.client).await },
            is_stripe_transient,
        )
        .await?;

        let first_item = subscription.items.data.first().ok_or_else(|| {
            StripeError::ClientError("Subscription has no items".into())
        })?;

        Ok(first_item.quantity.unwrap_or(1))
    }

    /// Update the seat count on a Cloud subscription.
    ///
    /// Retrieves the subscription, finds the first item, and sets its quantity.
    pub async fn update_seat_count(
        &self,
        subscription_id: &str,
        total_users: u64,
    ) -> Result<(), StripeError> {
        let subscription = kyomi_core::retry::retry_with_backoff_classified(
            || async { RetrieveSubscription::new(subscription_id).send(&self.client).await },
            is_stripe_transient,
        )
        .await?;

        let first_item = subscription.items.data.first().ok_or_else(|| {
            StripeError::ClientError("Subscription has no items".into())
        })?;

        let first_item_id = first_item.id.clone();
        kyomi_core::retry::retry_with_backoff_classified(
            || {
                let first_item_id = first_item_id.clone();
                async move {
                    UpdateSubscriptionItem::new(first_item_id)
                        .quantity(total_users)
                        .send(&self.client)
                        .await
                }
            },
            is_stripe_transient,
        )
        .await?;

        tracing::info!(
            subscription_id,
            total_users,
            "Updated seat count"
        );

        Ok(())
    }

    /// Cancel a subscription.
    ///
    /// If `at_period_end` is true, the subscription remains active until the
    /// end of the current billing period. If false, cancellation is immediate.
    pub async fn cancel_subscription(
        &self,
        subscription_id: &str,
        at_period_end: bool,
    ) -> Result<(), StripeError> {
        if at_period_end {
            UpdateSubscription::new(subscription_id)
                .cancel_at_period_end(true)
                .send(&self.client)
                .await?;
            tracing::info!(subscription_id, "Scheduled subscription cancellation at period end");
        } else {
            CancelSubscription::new(subscription_id)
                .send(&self.client)
                .await?;
            tracing::info!(subscription_id, "Immediately cancelled subscription");
        }

        Ok(())
    }

    /// Reactivate a subscription that was scheduled for cancellation
    /// (remove the `cancel_at_period_end` flag).
    pub async fn reactivate_subscription(
        &self,
        subscription_id: &str,
    ) -> Result<(), StripeError> {
        UpdateSubscription::new(subscription_id)
            .cancel_at_period_end(false)
            .send(&self.client)
            .await?;

        tracing::info!(subscription_id, "Reactivated subscription");
        Ok(())
    }

    // ── Webhook verification ─────────────────────────────────────────────

    /// Verify a Stripe webhook signature and parse the event.
    ///
    /// Uses the library's `Webhook::construct_event` which handles HMAC-SHA256
    /// verification and event deserialization from Stripe's current OpenAPI spec.
    pub fn construct_webhook_event(
        &self,
        payload: &str,
        sig_header: &str,
    ) -> Result<stripe_webhook::Event, stripe_webhook::WebhookError> {
        Webhook::construct_event(payload, sig_header, &self.webhook_secret)
    }

    // ── Subscription data parsing ────────────────────────────────────────

    /// Parse a Stripe `Subscription` object into our application-level
    /// `SubscriptionData`.
    ///
    /// All subscriptions are Cloud tier. User limit equals the quantity
    /// on the first subscription item.
    pub async fn parse_subscription_data(
        &self,
        subscription: &Subscription,
    ) -> Result<SubscriptionData, StripeError> {
        let items = &subscription.items.data;

        // Billing cycle from metadata (kept for display purposes)
        let billing_cycle = subscription.metadata.get("billing_cycle").cloned();

        // User limit = quantity of the first (and only) subscription item
        let user_limit = items
            .first()
            .and_then(|item| item.quantity)
            .unwrap_or(1) as i32;

        // Determine status
        let status = if subscription.cancel_at_period_end {
            "cancelled".to_string()
        } else {
            match subscription.status {
                SubscriptionStatus::Trialing => "trialing".to_string(),
                SubscriptionStatus::Active => "active".to_string(),
                SubscriptionStatus::PastDue => "past_due".to_string(),
                SubscriptionStatus::Canceled => "cancelled".to_string(),
                SubscriptionStatus::Unpaid => "cancelled".to_string(),
                _ => "active".to_string(),
            }
        };

        // Get period dates from the first subscription item
        // (current_period_start/end moved from Subscription to SubscriptionItem
        // in the Stripe API — the 1.0 crate reflects this change)
        let (period_start, period_end) = if let Some(first_item) = items.first() {
            (
                DateTime::from_timestamp(first_item.current_period_start, 0),
                DateTime::from_timestamp(first_item.current_period_end, 0),
            )
        } else {
            (None, None)
        };

        // Get customer ID
        let customer_id = match &subscription.customer {
            Expandable::Id(id) => id.to_string(),
            Expandable::Object(c) => c.id.to_string(),
        };

        Ok(SubscriptionData {
            tier: "cloud".to_string(),
            status,
            billing_cycle,
            user_limit,
            period_start,
            period_end,
            stripe_subscription_id: subscription.id.to_string(),
            stripe_customer_id: customer_id,
        })
    }

    // ── Invoices ─────────────────────────────────────────────────────────

    /// List recent invoices for a Stripe customer.
    pub async fn list_invoices(
        &self,
        customer_id: &str,
        limit: u64,
    ) -> Result<Vec<InvoiceData>, StripeError> {
        let invoices = ListInvoice::new()
            .customer(customer_id)
            .limit(limit as i64)
            .send(&self.client)
            .await?;

        let result = invoices
            .data
            .iter()
            .map(|inv| InvoiceData {
                invoice_id: inv.id.as_ref().map(|id| id.to_string()).unwrap_or_default(),
                amount_paid: inv.amount_paid,
                currency: inv.currency.to_string(),
                status: inv.status.as_ref().map(|s| format!("{s:?}").to_lowercase()),
                hosted_invoice_url: inv.hosted_invoice_url.clone(),
                invoice_pdf: inv.invoice_pdf.clone(),
                created: Some(inv.created),
            })
            .collect();

        Ok(result)
    }

    // ── Embedded Checkout ─────────────────────────────────────────────────

    /// Create an embedded Stripe Checkout session for a new subscription.
    ///
    /// Unlike hosted checkout, the user stays on our site. The returned
    /// `client_secret` is passed to Stripe.js `initEmbeddedCheckout()`.
    pub async fn create_embedded_checkout_session(
        &self,
        params: &EmbeddedCheckoutParams,
    ) -> Result<EmbeddedCheckoutResult, StripeError> {
        let line_items = vec![CreateCheckoutSessionLineItems {
            price: Some(params.price_id.clone()),
            quantity: Some(params.quantity),
            ..Default::default()
        }];

        let sub_metadata: std::collections::HashMap<String, String> = [
            ("workspace_id".to_string(), params.workspace_id.clone()),
            ("brand".to_string(), "kyomi".to_string()),
            ("app".to_string(), "kyomi".to_string()),
        ]
        .into_iter()
        .collect();

        let mut sub_data = CreateCheckoutSessionSubscriptionData {
            description: Some("Kyomi Cloud".to_string()),
            metadata: Some(sub_metadata),
            ..Default::default()
        };

        if params.trial_days > 0 {
            sub_data.trial_period_days = Some(params.trial_days);
        }

        let session_metadata: std::collections::HashMap<String, String> = [
            ("workspace_id".to_string(), params.workspace_id.clone()),
            ("brand".to_string(), "kyomi".to_string()),
        ]
        .into_iter()
        .collect();

        let session = CreateCheckoutSession::new()
            .customer(&params.customer_id)
            .mode(CheckoutSessionMode::Subscription)
            .ui_mode(stripe_shared::CheckoutSessionUiMode::Embedded)
            .redirect_on_completion(stripe_shared::CheckoutSessionRedirectOnCompletion::Never)
            .payment_method_types(vec![CreateCheckoutSessionPaymentMethodTypes::Card])
            .line_items(line_items)
            .subscription_data(sub_data)
            .metadata(session_metadata)
            .custom_text(CreateCheckoutSessionCustomText {
                submit: Some(CustomTextPositionParam::new("Subscribe to Kyomi Cloud")),
                ..Default::default()
            })
            .automatic_tax(CreateCheckoutSessionAutomaticTax {
                enabled: true,
                liability: None,
            })
            .tax_id_collection(CreateCheckoutSessionTaxIdCollection::new(true))
            .customer_update(CreateCheckoutSessionCustomerUpdate {
                name: Some(CreateCheckoutSessionCustomerUpdateName::Auto),
                address: Some(CreateCheckoutSessionCustomerUpdateAddress::Auto),
                ..Default::default()
            })
            .send(&self.client)
            .await?;

        tracing::info!(
            session_id = %session.id,
            workspace_id = %params.workspace_id,
            "Created embedded checkout session"
        );

        let client_secret = session.client_secret.ok_or_else(|| {
            StripeError::ClientError(
                "Embedded checkout session created but no client_secret returned".into(),
            )
        })?;

        Ok(EmbeddedCheckoutResult {
            session_id: session.id.to_string(),
            client_secret,
        })
    }

    /// Create an embedded payment checkout session (for bundle purchases).
    pub async fn create_embedded_payment_checkout_session(
        &self,
        params: &EmbeddedPaymentCheckoutParams,
    ) -> Result<EmbeddedCheckoutResult, StripeError> {
        let line_items = vec![CreateCheckoutSessionLineItems {
            price: Some(params.price_id.clone()),
            quantity: Some(params.quantity),
            ..Default::default()
        }];

        let session_metadata: std::collections::HashMap<String, String> = [
            ("workspace_id".to_string(), params.workspace_id.clone()),
            ("brand".to_string(), "kyomi".to_string()),
            ("purchase_type".to_string(), params.purchase_type.clone()),
            ("bundle_quantity".to_string(), params.quantity.to_string()),
        ]
        .into_iter()
        .collect();

        let session = CreateCheckoutSession::new()
            .customer(&params.customer_id)
            .mode(CheckoutSessionMode::Payment)
            .ui_mode(stripe_shared::CheckoutSessionUiMode::Embedded)
            .redirect_on_completion(stripe_shared::CheckoutSessionRedirectOnCompletion::Never)
            .payment_method_types(vec![CreateCheckoutSessionPaymentMethodTypes::Card])
            .line_items(line_items)
            .metadata(session_metadata)
            .automatic_tax(CreateCheckoutSessionAutomaticTax {
                enabled: true,
                liability: None,
            })
            .customer_update(CreateCheckoutSessionCustomerUpdate {
                address: Some(CreateCheckoutSessionCustomerUpdateAddress::Auto),
                ..Default::default()
            })
            .send(&self.client)
            .await
            .map_err(|e| {
                tracing::error!("Stripe embedded payment checkout failed: {e}");
                e
            })?;

        tracing::info!(
            session_id = %session.id,
            workspace_id = %params.workspace_id,
            purchase_type = %params.purchase_type,
            "Created embedded payment checkout session"
        );

        let client_secret = session.client_secret.ok_or_else(|| {
            StripeError::ClientError(
                "Embedded checkout session created but no client_secret returned".into(),
            )
        })?;

        Ok(EmbeddedCheckoutResult {
            session_id: session.id.to_string(),
            client_secret,
        })
    }

    /// Retrieve a checkout session's status (for verifying completion).
    pub async fn retrieve_checkout_session_status(
        &self,
        session_id: &str,
    ) -> Result<CheckoutSessionStatus, StripeError> {
        let session = RetrieveCheckoutSession::new(session_id.to_string())
            .send(&self.client)
            .await?;

        let status = session
            .status
            .map(|s| s.as_str().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let payment_status = session.payment_status.as_str().to_string();

        Ok(CheckoutSessionStatus {
            status,
            payment_status,
        })
    }

    // ── Billing Portal ───────────────────────────────────────────────────

    /// Create a Stripe Customer Portal session.
    ///
    /// Returns `(portal_url, session_id)` for the customer to manage billing.
    pub async fn create_portal_session(
        &self,
        customer_id: &str,
        return_url: &str,
    ) -> Result<(String, String), StripeError> {
        let portal = stripe_billing::billing_portal_session::CreateBillingPortalSession::new()
            .customer(customer_id)
            .return_url(return_url)
            .send(&self.client)
            .await?;

        tracing::info!(
            customer_id,
            session_id = %portal.id,
            "Created billing portal session"
        );

        Ok((portal.url, portal.id.to_string()))
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checkout_params_debug() {
        // Verify the struct derives Debug correctly
        let params = CheckoutParams {
            customer_id: "cus_test".to_string(),
            price_id: "price_test".to_string(),
            success_url: "https://example.com/success".to_string(),
            cancel_url: "https://example.com/cancel".to_string(),
            workspace_id: "ws-test".to_string(),
            quantity: 1,
            trial_days: 0,
        };
        let debug_str = format!("{params:?}");
        assert!(debug_str.contains("cus_test"));
    }

    #[test]
    fn test_embedded_checkout_result_serialization() {
        let result = EmbeddedCheckoutResult {
            client_secret: "cs_test_secret".to_string(),
            session_id: "cs_test_123".to_string(),
        };
        let json = serde_json::to_value(&result).expect("serialize");
        assert_eq!(json["client_secret"], "cs_test_secret");
        assert_eq!(json["session_id"], "cs_test_123");
    }

    #[test]
    fn test_checkout_session_status_serialization() {
        let status = CheckoutSessionStatus {
            status: "complete".to_string(),
            payment_status: "paid".to_string(),
        };
        let json = serde_json::to_value(&status).expect("serialize");
        assert_eq!(json["status"], "complete");
        assert_eq!(json["payment_status"], "paid");
    }

    #[test]
    fn test_subscription_data_serialization() {
        let data = SubscriptionData {
            tier: "cloud".to_string(),
            status: "active".to_string(),
            billing_cycle: Some("monthly".to_string()),
            user_limit: 3,
            period_start: None,
            period_end: None,
            stripe_subscription_id: "sub_test".to_string(),
            stripe_customer_id: "cus_test".to_string(),
        };
        let json = serde_json::to_value(&data).expect("serialize");
        assert_eq!(json["tier"], "cloud");
        assert_eq!(json["status"], "active");
        assert_eq!(json["user_limit"], 3);
    }

    // -- Stripe retry classification -----------------------------------------

    #[test]
    fn stripe_timeout_is_transient() {
        assert!(is_stripe_transient(&StripeError::Timeout));
    }

    #[test]
    fn stripe_client_error_is_transient() {
        assert!(is_stripe_transient(&StripeError::ClientError(
            "connection reset".into()
        )));
    }

    #[test]
    fn stripe_429_is_transient() {
        // 429 Too Many Requests — rate limiting is transient.
        assert!(is_stripe_transient(&StripeError::Stripe(
            Box::new(stripe_shared::ApiErrors {
                type_: stripe_shared::ApiErrorsType::ApiError,
                advice_code: None,
                charge: None,
                code: None,
                decline_code: None,
                doc_url: None,
                message: Some("Rate limit exceeded".into()),
                network_advice_code: None,
                network_decline_code: None,
                param: None,
                payment_intent: None,
                payment_method: None,
                payment_method_type: None,
                request_log_url: None,
                setup_intent: None,
                source: None,
            }),
            429,
        )));
    }

    #[test]
    fn stripe_503_is_transient() {
        // 503 Service Unavailable — upstream failure is transient.
        assert!(is_stripe_transient(&StripeError::Stripe(
            Box::new(stripe_shared::ApiErrors {
                type_: stripe_shared::ApiErrorsType::ApiError,
                advice_code: None,
                charge: None,
                code: None,
                decline_code: None,
                doc_url: None,
                message: Some("Service unavailable".into()),
                network_advice_code: None,
                network_decline_code: None,
                param: None,
                payment_intent: None,
                payment_method: None,
                payment_method_type: None,
                request_log_url: None,
                setup_intent: None,
                source: None,
            }),
            503,
        )));
    }

    #[test]
    fn stripe_400_is_not_transient() {
        // 400 Bad Request — permanent error.
        assert!(!is_stripe_transient(&StripeError::Stripe(
            Box::new(stripe_shared::ApiErrors {
                type_: stripe_shared::ApiErrorsType::InvalidRequestError,
                advice_code: None,
                charge: None,
                code: None,
                decline_code: None,
                doc_url: None,
                message: Some("Invalid request".into()),
                network_advice_code: None,
                network_decline_code: None,
                param: None,
                payment_intent: None,
                payment_method: None,
                payment_method_type: None,
                request_log_url: None,
                setup_intent: None,
                source: None,
            }),
            400,
        )));
    }

    #[test]
    fn stripe_401_is_not_transient() {
        // 401 Unauthorized — permanent error.
        assert!(!is_stripe_transient(&StripeError::Stripe(
            Box::new(stripe_shared::ApiErrors {
                type_: stripe_shared::ApiErrorsType::ApiError,
                advice_code: None,
                charge: None,
                code: None,
                decline_code: None,
                doc_url: None,
                message: Some("No such API key".into()),
                network_advice_code: None,
                network_decline_code: None,
                param: None,
                payment_intent: None,
                payment_method: None,
                payment_method_type: None,
                request_log_url: None,
                setup_intent: None,
                source: None,
            }),
            401,
        )));
    }

    #[test]
    fn stripe_json_deserialize_is_not_transient() {
        assert!(!is_stripe_transient(&StripeError::JSONDeserialize(
            "unexpected field".into()
        )));
    }

    #[test]
    fn stripe_config_error_is_not_transient() {
        assert!(!is_stripe_transient(&StripeError::ConfigError(
            "invalid key format".into()
        )));
    }
}
