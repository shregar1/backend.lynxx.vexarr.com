//! Cluster bus — fan-out of admin mutations to all gateway nodes.
//!
//! When the gateway runs as multiple nodes against a shared database, an
//! admin mutation on node A must also refresh the in-memory state on every
//! other node. This module defines the `ChangeEvent` enum and the
//! `ClusterBus` trait that backends implement: Postgres uses LISTEN/NOTIFY,
//! SQLite polls a `_sync_log` table.
//!
//! The flow on each node:
//!   1. Admin handler writes to the DB and updates local state.
//!   2. Handler publishes a `ChangeEvent` tagged with the local `node_id`.
//!   3. A subscriber task on every node receives the event, ignores its own
//!      (matched by `origin == node_id`), and refreshes the affected slice
//!      of in-memory state.

pub mod postgres;
pub mod sqlite;

use crate::error::GatewayError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use uuid::Uuid;

/// A bus event describing an entity-level change. The remote subscriber
/// uses these to know which slice of in-memory state to refresh; the actual
/// DB row is the source of truth and is re-fetched on receipt.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload")]
pub enum ChangeEvent {
    ProjectChanged {
        origin: Uuid,
    },
    ServiceCreated {
        id: Uuid,
        origin: Uuid,
    },
    ServiceUpdated {
        id: Uuid,
        origin: Uuid,
    },
    ServiceDeleted {
        id: Uuid,
        origin: Uuid,
    },
    RouteCreated {
        id: Uuid,
        origin: Uuid,
    },
    RouteUpdated {
        id: Uuid,
        origin: Uuid,
    },
    RouteDeleted {
        id: Uuid,
        origin: Uuid,
    },
    /// Any add/update/delete of a target. Listeners refetch all targets for
    /// the service and rebuild its load balancer.
    TargetChanged {
        service_id: Uuid,
        origin: Uuid,
    },
    /// Any plugin CRUD — listeners rebuild the entire plugin executor.
    PluginChanged {
        origin: Uuid,
    },
    /// Any consumer CRUD — currently a no-op on subscribers (consumers are
    /// read on-demand from the repo by the auth plugins).
    ConsumerChanged {
        origin: Uuid,
    },
    /// Emitted after `/reload` — listeners refetch all entities.
    FullSync {
        origin: Uuid,
    },
}

impl ChangeEvent {
    /// The node that originated this event. Used to ignore self-published
    /// events on the subscriber side.
    pub fn origin(&self) -> Uuid {
        match self {
            ChangeEvent::ProjectChanged { origin }
            | ChangeEvent::ServiceCreated { origin, .. }
            | ChangeEvent::ServiceUpdated { origin, .. }
            | ChangeEvent::ServiceDeleted { origin, .. }
            | ChangeEvent::RouteCreated { origin, .. }
            | ChangeEvent::RouteUpdated { origin, .. }
            | ChangeEvent::RouteDeleted { origin, .. }
            | ChangeEvent::TargetChanged { origin, .. }
            | ChangeEvent::PluginChanged { origin }
            | ChangeEvent::ConsumerChanged { origin }
            | ChangeEvent::FullSync { origin } => *origin,
        }
    }

    /// Short tag for log messages.
    pub fn kind(&self) -> &'static str {
        match self {
            ChangeEvent::ProjectChanged { .. } => "project.changed",
            ChangeEvent::ServiceCreated { .. } => "service.created",
            ChangeEvent::ServiceUpdated { .. } => "service.updated",
            ChangeEvent::ServiceDeleted { .. } => "service.deleted",
            ChangeEvent::RouteCreated { .. } => "route.created",
            ChangeEvent::RouteUpdated { .. } => "route.updated",
            ChangeEvent::RouteDeleted { .. } => "route.deleted",
            ChangeEvent::TargetChanged { .. } => "target.changed",
            ChangeEvent::PluginChanged { .. } => "plugin.changed",
            ChangeEvent::ConsumerChanged { .. } => "consumer.changed",
            ChangeEvent::FullSync { .. } => "full.sync",
        }
    }
}

/// Boxed async handler invoked once per remote event.
pub type ChangeHandler =
    Arc<dyn Fn(ChangeEvent) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Pub/sub bus implemented by each storage backend.
#[async_trait]
pub trait ClusterBus: Send + Sync {
    /// Publish an event to the cluster. Should never block on remote nodes —
    /// at most a single round-trip to the database.
    async fn publish(&self, event: ChangeEvent) -> Result<(), GatewayError>;

    /// Spawn a long-running task that streams events, calling `handler` for
    /// every event whose `origin` differs from `node_id`. Errors are logged
    /// internally and retried; the task lives until the returned join handle
    /// is dropped or aborted.
    fn subscribe(&self, node_id: Uuid, handler: ChangeHandler) -> tokio::task::JoinHandle<()>;
}

/// A no-op bus used in tests or when clustering is explicitly disabled.
/// `publish` succeeds silently and `subscribe` immediately returns a
/// completed task.
pub struct NoopBus;

#[async_trait]
impl ClusterBus for NoopBus {
    async fn publish(&self, _event: ChangeEvent) -> Result<(), GatewayError> {
        Ok(())
    }

    fn subscribe(&self, _node_id: Uuid, _handler: ChangeHandler) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async {})
    }
}
