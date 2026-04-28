use latex_extract::macros::expand_macros_cancellable;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[test]
fn expand_macros_honours_cancel_flag() {
    // Mutually-recursive macros would expand until max_passes or size cap
    // without cooperative cancellation. With the flag flipped after 10 ms,
    // expansion must abort before ~500 ms.
    let mut macros = HashMap::new();
    macros.insert("\\a".into(), "\\b\\b\\b".to_string());
    macros.insert("\\b".into(), "\\a\\a\\a".to_string());
    let input = "\\a".repeat(1000);

    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(10));
        flag.store(true, Ordering::Relaxed);
    });

    let start = Instant::now();
    let _out = expand_macros_cancellable(&input, &macros, Some(&cancel));
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "cancel did not short-circuit expansion (elapsed {:?})",
        start.elapsed()
    );
}

#[test]
fn expand_macros_without_cancel_still_works() {
    // Regression: the non-cancellable path (the old expand_macros API)
    // must behave identically — benign macros fully expand.
    use latex_extract::macros::expand_macros;
    let mut macros = HashMap::new();
    macros.insert("\\foo".into(), "bar".to_string());
    let out = expand_macros("\\foo baz \\foo", &macros);
    assert_eq!(out, "bar baz bar");
}
