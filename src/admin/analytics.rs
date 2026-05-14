use crate::admin::routes::AdminState;
use crate::error::GatewayError;
use crate::models::{MetricBucket, RateLimitBucket};
use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct AnalyticsQuery {
    pub range: Option<String>,
}

fn parse_range(range: &str) -> Duration {
    match range {
        "1h" => Duration::hours(1),
        "6h" => Duration::hours(6),
        "24h" => Duration::hours(24),
        "7d" => Duration::days(7),
        "30d" => Duration::days(30),
        _ => Duration::hours(24),
    }
}

fn granularity_for_range(range: &str) -> &'static str {
    match range {
        "1h" => "minute",
        "6h" => "minute",
        "24h" => "hour",
        "7d" => "hour",
        "30d" => "day",
        _ => "hour",
    }
}

#[derive(Serialize)]
pub struct AnalyticsResponse {
    pub summary: AnalyticsSummary,
    pub timeseries: Vec<MetricBucket>,
}

#[derive(Serialize)]
pub struct AnalyticsSummary {
    pub total_requests: i64,
    pub success_count: i64,
    pub error_4xx: i64,
    pub error_5xx: i64,
    pub avg_latency_ms: f64,
    pub p99_latency_ms: Option<i64>,
    pub bytes_in: i64,
    pub bytes_out: i64,
}

pub async fn service_analytics(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Query(q): Query<AnalyticsQuery>,
) -> Result<Json<AnalyticsResponse>, GatewayError> {
    let range_str = q.range.as_deref().unwrap_or("24h");
    let dur = parse_range(range_str);
    let granularity = granularity_for_range(range_str);
    let now = Utc::now();
    let from = now - dur;

    let buckets = state
        .repo
        .list_metric_buckets(&id, from, now, granularity)
        .await?;

    let mut total = 0i64;
    let mut success = 0i64;
    let mut e4xx = 0i64;
    let mut e5xx = 0i64;
    let mut lat_total = 0i64;
    let mut max_lat: Option<i64> = None;
    let mut bin = 0i64;
    let mut bout = 0i64;

    for b in &buckets {
        total += b.total_requests;
        success += b.success_count;
        e4xx += b.error_4xx;
        e5xx += b.error_5xx;
        lat_total += b.total_latency_ms;
        if let Some(m) = b.max_latency_ms {
            max_lat = Some(max_lat.map_or(m, |cur: i64| cur.max(m)));
        }
        bin += b.bytes_in;
        bout += b.bytes_out;
    }

    let avg_lat = if total > 0 {
        lat_total as f64 / total as f64
    } else {
        0.0
    };

    Ok(Json(AnalyticsResponse {
        summary: AnalyticsSummary {
            total_requests: total,
            success_count: success,
            error_4xx: e4xx,
            error_5xx: e5xx,
            avg_latency_ms: avg_lat,
            p99_latency_ms: max_lat,
            bytes_in: bin,
            bytes_out: bout,
        },
        timeseries: buckets,
    }))
}

#[derive(Serialize)]
pub struct RateLimitInfo {
    pub consumer_id: Option<Uuid>,
    pub allowed: i64,
    pub throttled: i64,
    pub window_start: String,
}

pub async fn service_rate_limits(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<RateLimitInfo>>, GatewayError> {
    let now = Utc::now();
    let from = now - Duration::hours(1);
    let buckets = state.repo.list_rate_limit_buckets(&id, from, now).await?;

    let info: Vec<RateLimitInfo> = buckets
        .into_iter()
        .map(|b| RateLimitInfo {
            consumer_id: b.consumer_id,
            allowed: b.allowed,
            throttled: b.throttled,
            window_start: b.bucket_ts.to_rfc3339(),
        })
        .collect();

    Ok(Json(info))
}

#[derive(Serialize)]
pub struct RateLimitStats {
    pub total_allowed: i64,
    pub total_throttled: i64,
    pub timeseries: Vec<RateLimitBucket>,
}

pub async fn service_rate_limit_stats(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Query(q): Query<AnalyticsQuery>,
) -> Result<Json<RateLimitStats>, GatewayError> {
    let range_str = q.range.as_deref().unwrap_or("24h");
    let dur = parse_range(range_str);
    let now = Utc::now();
    let from = now - dur;

    let buckets = state.repo.list_rate_limit_buckets(&id, from, now).await?;

    let mut allowed = 0i64;
    let mut throttled = 0i64;
    for b in &buckets {
        allowed += b.allowed;
        throttled += b.throttled;
    }

    Ok(Json(RateLimitStats {
        total_allowed: allowed,
        total_throttled: throttled,
        timeseries: buckets,
    }))
}

#[derive(Serialize)]
pub struct GeoStats {
    pub countries: Vec<GeoCountry>,
    pub domestic_pct: f64,
}

#[derive(Serialize)]
pub struct GeoCountry {
    pub country_code: String,
    pub city: String,
    pub request_count: i64,
}

pub async fn service_geo(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Query(q): Query<AnalyticsQuery>,
) -> Result<Json<GeoStats>, GatewayError> {
    let range_str = q.range.as_deref().unwrap_or("24h");
    let dur = parse_range(range_str);
    let now = Utc::now();
    let from = now - dur;

    let buckets = state.repo.list_geo_buckets(&id, from, now).await?;

    let mut by_country: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    let mut countries_list = Vec::new();
    let mut grand_total = 0i64;

    for b in &buckets {
        *by_country.entry(b.country_code.clone()).or_default() += b.request_count;
        grand_total += b.request_count;
        countries_list.push(GeoCountry {
            country_code: b.country_code.clone(),
            city: b.city.clone(),
            request_count: b.request_count,
        });
    }

    let top_country_count = by_country.values().max().copied().unwrap_or(0);
    let domestic_pct = if grand_total > 0 {
        top_country_count as f64 / grand_total as f64 * 100.0
    } else {
        0.0
    };

    Ok(Json(GeoStats {
        countries: countries_list,
        domestic_pct,
    }))
}
