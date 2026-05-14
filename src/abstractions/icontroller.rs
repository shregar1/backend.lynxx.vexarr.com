use axum::Router;

/// Interface for HTTP controller modules that contribute routes to the
/// admin API. Each controller exposes read (viewer-level) and mutation
/// (operator-level) route groups.
pub trait IController {
    type State: Clone + Send + Sync + 'static;
    fn read_routes() -> Router<Self::State>;
    fn mutation_routes() -> Router<Self::State>;
}
