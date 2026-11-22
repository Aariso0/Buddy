use buddy_core::clock::SessionClock;

#[test]
fn clock_never_moves_backwards() {
    let clock = SessionClock::new();
    let mut last = clock.now();
    for _ in 0..1000 {
        let now = clock.now();
        assert!(now >= last);
        last = now;
    }
}
