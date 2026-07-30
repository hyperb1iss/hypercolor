use super::{checked_cell_count, try_zone_color_buffer};

#[test]
fn screen_zone_cell_count_rejects_overflow() {
    assert_eq!(checked_cell_count(usize::MAX, 2), None);
}

#[test]
fn screen_zone_color_buffer_rejects_impossible_capacity() {
    assert!(try_zone_color_buffer(usize::MAX).is_none());
}
