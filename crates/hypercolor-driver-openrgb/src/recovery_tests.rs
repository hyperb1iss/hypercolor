use super::*;

#[tokio::test]
async fn delayed_release_of_old_policy_cannot_remove_its_replacement() {
    let pool = EndpointPool::default();
    let endpoint = "127.0.0.1:6742".parse().expect("endpoint");
    let original = OpenRgbConfig::default();
    let mut old_pin = pool.acquire(endpoint, &original).await;
    let old = Arc::clone(&old_pin.connection);
    // Model the gap between synchronous owner release and async pool cleanup.
    old_pin.release_pin();
    let updated = OpenRgbConfig {
        read_timeout_ms: 100,
        ..original
    };
    let replacement_pin = pool.acquire(endpoint, &updated).await;
    let replacement = Arc::clone(&replacement_pin.connection);
    assert!(!Arc::ptr_eq(&old, &replacement));
    assert_eq!(replacement.config, updated);
    pool.release_if_idle(&old).await;
    let pooled = pool.open(endpoint).expect("replacement remains pooled");
    assert!(Arc::ptr_eq(&pooled, &replacement));
    let second = pool.acquire(endpoint, &updated).await;
    assert!(Arc::ptr_eq(&second.connection, &replacement));
    second.release().await;
    replacement_pin.release().await;
    assert!(pool.open(endpoint).is_none());
}

#[tokio::test]
async fn concurrent_acquirers_wait_for_the_previous_generation_to_close() {
    let pool = Arc::new(EndpointPool::default());
    let endpoint = "127.0.0.1:6742".parse().expect("endpoint");
    let original = OpenRgbConfig::default();
    let mut old_pin = pool.acquire(endpoint, &original).await;
    let old = Arc::clone(&old_pin.connection);
    old_pin.release_pin();
    let old_link = old.link.lock().await;
    let updated = OpenRgbConfig {
        read_timeout_ms: 100,
        ..original
    };
    let first = {
        let pool = Arc::clone(&pool);
        let updated = updated.clone();
        tokio::spawn(async move { pool.acquire(endpoint, &updated).await })
    };
    tokio::task::yield_now().await;
    let replacement = pool.open(endpoint).expect("replacement inserted");
    assert!(!Arc::ptr_eq(&old, &replacement));
    let second = {
        let pool = Arc::clone(&pool);
        tokio::spawn(async move { pool.acquire(endpoint, &updated).await })
    };
    tokio::task::yield_now().await;
    assert!(!first.is_finished());
    assert!(!second.is_finished());
    drop(old_link);
    let first = first.await.expect("first acquirer");
    let second = second.await.expect("second acquirer");
    assert!(Arc::ptr_eq(&first.connection, &second.connection));
    pool.release_if_idle(&old).await;
    assert!(Arc::ptr_eq(
        &pool.open(endpoint).expect("live replacement"),
        &first.connection
    ));
    first.release().await;
    second.release().await;
}

#[tokio::test]
async fn cancelling_acquisition_releases_the_pin_while_predecessor_is_busy() {
    let pool = Arc::new(EndpointPool::default());
    let endpoint = "127.0.0.1:6742".parse().expect("endpoint");
    let original = OpenRgbConfig::default();
    let mut old_pin = pool.acquire(endpoint, &original).await;
    let old = Arc::clone(&old_pin.connection);
    old_pin.release_pin();
    let old_link = old.link.lock().await;
    let updated = OpenRgbConfig {
        read_timeout_ms: 100,
        ..original
    };
    let acquiring = {
        let pool = Arc::clone(&pool);
        let updated = updated.clone();
        tokio::spawn(async move { pool.acquire(endpoint, &updated).await })
    };
    tokio::task::yield_now().await;
    let replacement = pool.open(endpoint).expect("replacement inserted");
    assert_eq!(replacement.pins.load(Ordering::Acquire), 1);
    assert!(!acquiring.is_finished());
    acquiring.abort();
    assert!(matches!(acquiring.await, Err(error) if error.is_cancelled()));
    assert_eq!(replacement.pins.load(Ordering::Acquire), 0);
    drop(old_link);
    let next_config = OpenRgbConfig {
        read_timeout_ms: 200,
        ..updated
    };
    let next = pool.acquire(endpoint, &next_config).await;
    assert_eq!(next.connection.config, next_config);
    next.release().await;
}
