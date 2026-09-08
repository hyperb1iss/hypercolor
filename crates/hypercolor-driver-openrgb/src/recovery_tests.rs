use super::*;

#[tokio::test]
async fn delayed_release_of_old_policy_cannot_remove_its_replacement() {
    let pool = EndpointPool::default();
    let endpoint = "127.0.0.1:6742".parse().expect("endpoint");
    let original = OpenRgbConfig::default();
    let old = pool.acquire(endpoint, &original).await;
    // Model the gap between synchronous owner release and async pool cleanup.
    old.pins.fetch_sub(1, Ordering::AcqRel);
    let updated = OpenRgbConfig {
        read_timeout_ms: 100,
        ..original
    };
    let replacement = pool.acquire(endpoint, &updated).await;
    assert!(!Arc::ptr_eq(&old, &replacement));
    assert_eq!(replacement.config, updated);
    pool.release_if_idle(&old).await;
    let pooled = pool.open(endpoint).expect("replacement remains pooled");
    assert!(Arc::ptr_eq(&pooled, &replacement));
    let second = pool.acquire(endpoint, &updated).await;
    assert!(Arc::ptr_eq(&second, &replacement));
    pool.unpin(&second).await;
    pool.unpin(&replacement).await;
    assert!(pool.open(endpoint).is_none());
}
