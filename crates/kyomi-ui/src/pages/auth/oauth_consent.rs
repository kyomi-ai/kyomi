// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server-rendered MCP OAuth consent. The form submits without JavaScript so
//! authorization remains explicit even before the client bundle loads.

use leptos::prelude::*;

use crate::components::{Alert, AlertDescription, AlertVariant, Button, ButtonVariant};
use crate::pages::auth::auth_layout::AuthLayout;

#[component]
pub fn OAuthConsentPage(
    client_name: String,
    account: String,
    workspace: String,
    callback_origin: String,
    transaction: String,
    csrf: String,
) -> impl IntoView {
    view! {
        <AuthLayout
            title=Signal::derive(|| "Allow MCP access?".to_string())
            subtitle=Signal::derive(|| "Review this app's request to access Kyomi.".to_string())
        >
            <div class="space-y-6">
                <Alert variant=AlertVariant::Warning>
                    <AlertDescription>
                        "Kyomi has not verified this app's name or callback. Only allow access if you trust both."
                    </AlertDescription>
                </Alert>
                <div class="rounded-lg border border-border bg-card p-6 space-y-4">
                    <p class="text-base text-foreground">
                        <strong>{client_name}</strong>
                        " wants access to your Kyomi account."
                    </p>
                    <dl class="space-y-3 text-sm">
                        <div>
                            <dt class="font-medium text-muted-foreground">"Account"</dt>
                            <dd class="text-foreground break-all">{account}</dd>
                        </div>
                        <div>
                            <dt class="font-medium text-muted-foreground">"Workspace"</dt>
                            <dd class="text-foreground break-all">{workspace}</dd>
                        </div>
                        <div>
                            <dt class="font-medium text-muted-foreground">"Access requested"</dt>
                            <dd class="text-foreground">"Kyomi account access, including MCP tools, in this workspace"</dd>
                        </div>
                        <div>
                            <dt class="font-medium text-muted-foreground">"Callback"</dt>
                            <dd class="font-mono text-foreground break-all">{callback_origin}</dd>
                        </div>
                    </dl>
                </div>
                <div class="flex gap-3">
                    <form method="post" action="/api/v1/oauth/authorize" class="flex-1">
                        <input type="hidden" name="transaction" value=transaction.clone()/>
                        <input type="hidden" name="csrf" value=csrf.clone()/>
                        <input type="hidden" name="decision" value="allow"/>
                        <Button button_type="submit" class="w-full">"Allow"</Button>
                    </form>
                    <form method="post" action="/api/v1/oauth/authorize" class="flex-1">
                        <input type="hidden" name="transaction" value=transaction/>
                        <input type="hidden" name="csrf" value=csrf/>
                        <input type="hidden" name="decision" value="deny"/>
                        <Button button_type="submit" variant=ButtonVariant::Outline class="w-full">
                            "Deny"
                        </Button>
                    </form>
                </div>
            </div>
        </AuthLayout>
    }
}
