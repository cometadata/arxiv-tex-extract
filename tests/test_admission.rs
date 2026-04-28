use latex_extract::admission::AdmissionControl;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[test]
fn semaphore_blocks_beyond_limit() {
    let ac = Arc::new(AdmissionControl::new(2));
    let p1 = ac.acquire_owned();
    let _p2 = ac.acquire_owned();

    let ac2 = Arc::clone(&ac);
    let start = Instant::now();
    let t = std::thread::spawn(move || {
        // Blocks until p1 is released; proves the semaphore is honoured.
        let _p3 = ac2.acquire_owned();
    });

    // Give the other thread time to block.
    std::thread::sleep(Duration::from_millis(50));
    drop(p1);
    t.join().unwrap();

    assert!(
        start.elapsed() >= Duration::from_millis(50),
        "third acquire did not wait (elapsed {:?})",
        start.elapsed()
    );
}

#[test]
fn permit_release_on_drop_increments_count() {
    let ac = Arc::new(AdmissionControl::new(1));
    {
        let _p = ac.acquire_owned();
        assert_eq!(ac.available_permits(), 0);
    }
    assert_eq!(ac.available_permits(), 1);
}
