//! Keeps successful table evaluations in a bounded invocation-local LRU.

use std::{
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
};

use bytes::Bytes;
use lru::LruCache;
#[cfg(test)]
use reqwest::Url;

#[cfg(test)]
use crate::api::types::SystemOneRequest;
use crate::{api::types::SystemOneResponse, config::CacheLimits};

/// Includes the service root and a canonical complete request body, never credentials.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct RequestKey {
    root: Arc<str>,
    body: Bytes,
}

impl std::fmt::Debug for RequestKey {
    /// Redacts state and question bytes even in assertion diagnostics.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RequestKey(<redacted>)")
    }
}

impl RequestKey {
    /// Shares the canonical serialized body with retryable HTTP attempts.
    pub(crate) fn from_body(root: Arc<str>, body: Bytes) -> Self {
        Self { root, body }
    }

    /// Encodes a request for key-equivalence tests.
    #[cfg(test)]
    pub(crate) fn new(root: &Url, request: &SystemOneRequest) -> Self {
        Self::from_body(
            Arc::from(root.as_str()),
            Bytes::from(serde_json::to_vec(request).unwrap()),
        )
    }

    /// Reports the retained size of the key bytes for approximate cache accounting.
    fn byte_len(&self) -> usize {
        8 + self.root.len() + self.body.len()
    }
}

/// Holds the server result and local identity of one logical evaluation.
#[derive(Debug)]
pub(crate) struct SharedResponse {
    /// Complete validated response returned by the service.
    pub(crate) response: SystemOneResponse,
    /// Stable local identity shared by retries, waiters, and cache hits.
    pub(crate) request_id: String,
}

impl SharedResponse {
    /// Attaches a process-unique identity to a newly completed logical evaluation.
    pub(crate) fn new(response: SystemOneResponse, request_id: String) -> Self {
        Self {
            response,
            request_id,
        }
    }

    /// Estimates cache-retained bytes beyond the canonical key.
    fn byte_len(&self) -> usize {
        serde_json::to_vec(&self.response)
            .expect("validated Jev responses serialize as JSON")
            .len()
            .saturating_add(self.request_id.len())
    }
}

/// Allocates distinct identities without retaining request bodies or credentials.
pub(crate) fn next_request_id() -> String {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    format!("jev-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed))
}

/// Pairs one cached result with its deterministic approximate byte weight.
struct CacheEntry {
    result: Arc<SharedResponse>,
    weight: usize,
}

/// Stores only successful outcomes and evicts least-recently-used entries by two limits.
pub(crate) struct CompletedCache {
    entries: LruCache<RequestKey, CacheEntry>,
    limits: CacheLimits,
    approx_bytes: usize,
}

impl CompletedCache {
    /// Creates an empty cache owned by exactly one table invocation.
    pub(crate) fn new(limits: CacheLimits) -> Self {
        Self {
            entries: LruCache::unbounded(),
            limits,
            approx_bytes: 0,
        }
    }

    /// Returns a successful result and refreshes its recency when present.
    pub(crate) fn get(&mut self, key: &RequestKey) -> Option<Arc<SharedResponse>> {
        self.entries.get(key).map(|entry| Arc::clone(&entry.result))
    }

    /// Retains an eligible success; oversized results bypass without evicting peers.
    pub(crate) fn insert(&mut self, key: RequestKey, result: Arc<SharedResponse>) {
        // This is an estimate of retained cache data, not an upper bound on process RSS.
        const ENTRY_OVERHEAD: usize = 128;
        let weight = key
            .byte_len()
            .saturating_add(result.byte_len())
            .saturating_add(ENTRY_OVERHEAD);
        if weight > self.limits.max_approx_bytes.get() {
            return;
        }
        if let Some(previous) = self.entries.push(key, CacheEntry { result, weight }) {
            self.approx_bytes = self.approx_bytes.saturating_sub(previous.1.weight);
        }
        self.approx_bytes = self.approx_bytes.saturating_add(weight);
        while self.entries.len() > self.limits.max_entries.get()
            || self.approx_bytes > self.limits.max_approx_bytes.get()
        {
            if let Some((_, evicted)) = self.entries.pop_lru() {
                self.approx_bytes -= evicted.weight;
            }
        }
    }

    /// Reports retained cache entries for scheduling diagnostics and tests.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }

    /// Reports the sum of accounted key/result/provenance bytes and fixed overhead.
    #[cfg(test)]
    fn approx_bytes(&self) -> usize {
        self.approx_bytes
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, num::NonZeroUsize, sync::Arc};

    use reqwest::Url;
    use serde_json::json;

    use crate::{
        api::types::{Question, SystemOneRequest, SystemOneResponse},
        config::CacheLimits,
    };

    use super::{CompletedCache, RequestKey, SharedResponse};
    use crate::api::client::PreparedRequest;

    /// Builds a valid request while varying only the state under test.
    fn request(state: serde_json::Value) -> SystemOneRequest {
        SystemOneRequest {
            state,
            model: "jev-latest".into(),
            questions: BTreeMap::from([(
                "q".into(),
                Question::Noul {
                    instructions: None,
                    criteria: None,
                },
            )]),
        }
    }

    /// Builds a successful response with a distinguishable local identity.
    fn result() -> Arc<SharedResponse> {
        let response: SystemOneResponse = serde_json::from_value(json!({
            "model": "jev-fixed", "answers": {"q": {"type": "noul", "noul": 0.5}},
            "usage": {"input_tokens": 1, "output_tokens": 1}
        }))
        .unwrap();
        Arc::new(SharedResponse::new(response, "jev-test".into()))
    }

    /// Creates independently bounded cache limits for small fixtures.
    fn limits(entries: usize, bytes: usize) -> CacheLimits {
        CacheLimits {
            max_entries: NonZeroUsize::new(entries).unwrap(),
            max_approx_bytes: NonZeroUsize::new(bytes).unwrap(),
        }
    }

    /// Ignores JSON object key order while retaining array order and missing/null distinctions.
    #[test]
    fn canonical_keys_include_complete_body_and_service_root() {
        let root = Url::parse("https://example.test/api/").unwrap();
        let first = RequestKey::new(&root, &request(json!({"a": 1, "b": 2})));
        let reordered = RequestKey::new(&root, &request(json!({"b": 2, "a": 1})));
        assert_eq!(first, reordered);
        assert_ne!(first, RequestKey::new(&root, &request(json!([1, 2]))));
        assert_ne!(
            first,
            RequestKey::new(&root, &request(json!({"a": 1, "b": null})))
        );
        assert_ne!(first, RequestKey::new(&root, &request(json!({"a": 1}))));
        assert_ne!(
            first,
            RequestKey::new(
                &Url::parse("https://other.test/api/").unwrap(),
                &request(json!({"a": 1, "b": 2}))
            )
        );
        let mut changed = request(json!({"a": 1, "b": 2}));
        changed.model = "jev-fixed".into();
        assert_ne!(first, RequestKey::new(&root, &changed));
        changed.model = "jev-latest".into();
        changed.questions.insert(
            "q".into(),
            Question::Noul {
                instructions: Some(json!("different")),
                criteria: None,
            },
        );
        assert_ne!(first, RequestKey::new(&root, &changed));
        changed.questions.insert(
            "q".into(),
            Question::Noul {
                instructions: Some(json!(null)),
                criteria: None,
            },
        );
        assert_ne!(first, RequestKey::new(&root, &changed));
        assert_ne!(
            RequestKey::new(&root, &request(json!([1, 2]))),
            RequestKey::new(&root, &request(json!([2, 1])))
        );
    }

    /// Shares the prepared JSON allocation between the key and HTTP body.
    #[test]
    fn canonical_key_reuses_prepared_body_bytes() {
        let prepared = PreparedRequest::new(request(json!({"message": "hello"}))).unwrap();
        let key = RequestKey::from_body(Arc::from("https://example.test/"), prepared.body.clone());
        assert_eq!(key.body.as_ptr(), prepared.body.as_ptr());
        assert_eq!(key.body.len(), prepared.body.len());
    }

    /// Refreshes hit recency and evicts independently under the entry limit.
    #[test]
    fn lru_refreshes_and_evicts_by_entry_count() {
        let root = Url::parse("https://example.test/").unwrap();
        let keys = [0, 1, 2].map(|value| RequestKey::new(&root, &request(json!({"id": value}))));
        let mut cache = CompletedCache::new(limits(2, 1_000_000));
        let first = result();
        cache.insert(keys[0].clone(), Arc::clone(&first));
        cache.insert(keys[1].clone(), result());
        assert!(Arc::ptr_eq(&cache.get(&keys[0]).unwrap(), &first));
        cache.insert(keys[2].clone(), result());
        assert!(cache.get(&keys[1]).is_none());
        assert_eq!(cache.len(), 2);
    }

    /// Keeps prior entries when a single successful result is too large to retain.
    #[test]
    fn byte_pressure_and_oversized_bypass() {
        let root = Url::parse("https://example.test/").unwrap();
        let keys = [0, 1, 2].map(|value| RequestKey::new(&root, &request(json!({"id": value}))));
        let sample = keys[0].byte_len() + result().byte_len() + 128;
        let mut cache = CompletedCache::new(limits(10, sample * 2));
        cache.insert(keys[0].clone(), result());
        cache.insert(keys[1].clone(), result());
        assert_eq!(cache.len(), 2);
        cache.insert(keys[2].clone(), result());
        assert_eq!(cache.len(), 2);
        assert!(cache.approx_bytes() <= sample * 2);
        let huge = RequestKey::new(&root, &request(json!({"text": "x".repeat(sample * 3)})));
        cache.insert(huge.clone(), result());
        assert!(cache.get(&huge).is_none());
        assert_eq!(cache.len(), 2);
    }
}
