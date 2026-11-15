// SPDX-License-Identifier: AGPL-3.0-or-later

//! Ambient Kyomi Connect transport for catalog indexing.
//!
//! Connect is a *transport*, like TCP vs QUIC: it changes how a provider
//! reaches the database, never which indexing logic runs (KYO-967). The
//! registry is therefore an ambient service installed once at process startup
//! and consulted only by provider creation in
//! [`crate::catalog::traits::index_catalog_sql`] — callers of
//! [`CatalogIndexingService::index_datasource`] (scheduler, manual refresh,
//! post-create, alerts) supply no transport wiring and so cannot silently skip
//! Connect datasources by omitting it.
//!
//! [`CatalogIndexingService::index_datasource`]: crate::catalog::indexing_service::CatalogIndexingService::index_datasource

use std::sync::RwLock;
use std::time::Duration;

use kyomi_datasource_server::{ConnectProvider, ConnectRegistry, DatasourceProvider};

/// Timeout for each discovery query sent through the Connect tunnel.
pub const CONNECT_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(120);

static CONNECT_REGISTRY: RwLock<Option<ConnectRegistry>> = RwLock::new(None);

/// Install the process-wide Connect registry. Call once at startup, before any
/// catalog indexing can run. A later call replaces the earlier registry.
pub fn install_connect_registry(registry: ConnectRegistry) {
    *CONNECT_REGISTRY
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(registry);
}

/// The installed registry, if any.
pub fn connect_registry() -> Option<ConnectRegistry> {
    CONNECT_REGISTRY
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Build the Connect-transport provider for a datasource.
///
/// Returns `None` when no registry has been installed (a startup wiring
/// error the caller must report as a visible failure, not a silent skip).
pub fn connect_provider(datasource_config_id: &str) -> Option<Box<dyn DatasourceProvider>> {
    connect_registry().map(|registry| {
        Box::new(ConnectProvider::with_timeout(
            registry,
            datasource_config_id.to_string(),
            CONNECT_DISCOVERY_TIMEOUT,
        )) as Box<dyn DatasourceProvider>
    })
}
