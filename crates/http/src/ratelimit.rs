//! Bucket-aware rate limiter driven by Discord's `X-RateLimit-*` headers.
//!
//! Routes are mapped to buckets via `X-RateLimit-Bucket`; each bucket is scoped
//! by its major parameter (channel / guild / webhook id) as Discord does.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use reqwest::header::HeaderMap;

#[derive(Debug, Clone)]
struct BucketState {
    remaining: u32,
    reset_at: Instant,
}

#[derive(Default)]
pub struct RateLimiter {
    route_to_bucket: Mutex<HashMap<String, String>>,
    buckets: Mutex<HashMap<String, BucketState>>,
    global_until: Mutex<Option<Instant>>,
}

/// Route key: method + path with non-major snowflakes replaced, e.g.
/// `GET /channels/123/messages/:id` keeps the channel id (major) but drops the message id.
pub fn route_key(method: &str, path: &str) -> (String, String) {
    let mut major = String::new();
    let mut out = String::with_capacity(path.len());
    let mut prev = "";
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        out.push('/');
        let is_id = !seg.is_empty() && seg.bytes().all(|b| b.is_ascii_digit());
        if is_id {
            if matches!(prev, "channels" | "guilds" | "webhooks") && major.is_empty() {
                major = seg.to_string();
                out.push_str(seg);
            } else {
                out.push_str(":id");
            }
        } else if prev == "reactions" {
            out.push_str(":emoji");
        } else {
            out.push_str(seg);
        }
        prev = seg;
    }
    (format!("{method} {out}"), major)
}

impl RateLimiter {
    /// Sleeps until a request on this route is allowed.
    pub async fn acquire(&self, method: &str, path: &str) {
        loop {
            let wait = {
                let now = Instant::now();
                let global = self.global_until.lock().filter(|t| *t > now).map(|t| t - now);
                let (key, major) = route_key(method, path);
                let bucket_wait = self.route_to_bucket.lock().get(&key).cloned().and_then(|b| {
                    let mut buckets = self.buckets.lock();
                    let st = buckets.get_mut(&format!("{b}:{major}"))?;
                    if st.reset_at <= now {
                        return None;
                    }
                    if st.remaining == 0 {
                        Some(st.reset_at - now)
                    } else {
                        // Optimistically reserve a slot so concurrent requests don't all fire.
                        st.remaining -= 1;
                        None
                    }
                });
                global.max(bucket_wait)
            };
            match wait {
                Some(d) if d > Duration::ZERO => {
                    tracing::debug!(?d, path, "rate limit: waiting");
                    tokio::time::sleep(d.min(Duration::from_secs(60))).await;
                }
                _ => return,
            }
        }
    }

    pub fn update(&self, method: &str, path: &str, headers: &HeaderMap) {
        let get = |n: &str| headers.get(n).and_then(|v| v.to_str().ok());
        let Some(bucket) = get("x-ratelimit-bucket") else { return };
        let (key, major) = route_key(method, path);
        self.route_to_bucket.lock().insert(key, bucket.to_string());
        let remaining = get("x-ratelimit-remaining").and_then(|v| v.parse().ok()).unwrap_or(1);
        let reset_after: f64 = get("x-ratelimit-reset-after").and_then(|v| v.parse().ok()).unwrap_or(0.0);
        self.buckets.lock().insert(
            format!("{bucket}:{major}"),
            BucketState { remaining, reset_at: Instant::now() + Duration::from_secs_f64(reset_after) },
        );
    }

    pub fn set_global(&self, retry_after: f64) {
        *self.global_until.lock() = Some(Instant::now() + Duration::from_secs_f64(retry_after));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        assert_eq!(route_key("GET", "/channels/1/messages/2"), ("GET /channels/1/messages/:id".into(), "1".into()));
        assert_eq!(route_key("PUT", "/channels/1/messages/2/reactions/%F0%9F%91%8D/@me").0, "PUT /channels/1/messages/:id/reactions/:emoji/@me");
        assert_eq!(route_key("GET", "/users/@me").0, "GET /users/@me");
    }
}
