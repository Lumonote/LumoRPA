use lumo_voice::{AudioFrame, PreRollBuffer, TARGET_SAMPLE_RATE};

#[test]
fn averages_channels() {
    assert_eq!(
        AudioFrame::from_i16_interleaved(&[100, 300, -100, 100], 2).samples,
        vec![200, 0]
    );
}
#[test]
fn converts_and_clamps_f32() {
    assert_eq!(
        AudioFrame::from_f32_interleaved(&[2.0, -2.0, 0.5], 1).samples,
        vec![i16::MAX, i16::MIN, 16384]
    );
}
#[test]
fn bounded_and_ordered_drain() {
    let mut b = PreRollBuffer::new(3);
    b.push(AudioFrame {
        samples: vec![1, 2],
    });
    b.push(AudioFrame {
        samples: vec![3, 4],
    });
    assert_eq!(b.len(), 3);
    assert_eq!(b.drain_after_wake().samples, vec![2, 3, 4]);
    assert!(b.is_empty());
}
#[test]
fn two_second_capacity() {
    let mut b = PreRollBuffer::two_seconds();
    b.push(AudioFrame {
        samples: vec![1; (TARGET_SAMPLE_RATE as usize) * 3],
    });
    assert_eq!(b.len(), (TARGET_SAMPLE_RATE as usize) * 2);
}
#[test]
fn clear_empties_and_releases_no_samples() {
    let mut b = PreRollBuffer::new(4);
    b.push(AudioFrame {
        samples: vec![9, 8],
    });
    b.clear();
    assert!(b.is_empty());
    assert!(b.drain_after_wake().samples.is_empty());
}
