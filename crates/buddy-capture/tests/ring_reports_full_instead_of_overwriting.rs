mod common;

#[test]
fn ring_reports_full_instead_of_overwriting() {
    let ring = buddy_capture::ring::RingBuffer::new(4, 2);
    assert_eq!(ring.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]), Ok(()));
    assert!(matches!(
        ring.push(&[9.0, 10.0]),
        Err(buddy_capture::ring::RingError::Full)
    ));
    let mut out = [0.0; 3];
    assert_eq!(ring.pop(&mut out), Some(3));
    assert_eq!(out, [1.0, 2.0, 3.0]);
    assert_eq!(ring.push(&[9.0, 10.0, 11.0]), Ok(()));
    let mut rest = [0.0; 8];
    assert_eq!(ring.pop(&mut rest), Some(8));
    assert_eq!(&rest[..8], &[4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0]);
    assert_eq!(ring.pop(&mut rest), None);
}
