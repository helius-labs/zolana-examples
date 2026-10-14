use dvp_program::{error::DvpError, instructions::shared::check_settle_timestamp, TIME_WINDOW};
use pinocchio::error::ProgramError;

const NOW: i64 = 1_000;

fn out_of_window() -> Result<(), ProgramError> {
    Err(DvpError::TimestampOutOfWindow.into())
}

#[test]
fn settle_window_boundary() {
    assert_eq!(check_settle_timestamp(NOW, NOW), Ok(()));
    assert_eq!(check_settle_timestamp(NOW, NOW - TIME_WINDOW), Ok(()));
    assert_eq!(
        check_settle_timestamp(NOW, NOW - TIME_WINDOW - 1),
        out_of_window()
    );
    assert_eq!(check_settle_timestamp(NOW, NOW + 1), out_of_window());
}

#[test]
fn settle_window_rejects_negative_timestamp() {
    assert_eq!(check_settle_timestamp(NOW, -1), out_of_window());
    // Near the epoch the window reaches below zero; negatives stay rejected.
    assert_eq!(check_settle_timestamp(10, -1), out_of_window());
    assert_eq!(check_settle_timestamp(10, 0), Ok(()));
}

#[test]
fn settle_window_saturates_at_i64_min() {
    assert_eq!(check_settle_timestamp(i64::MIN, 0), out_of_window());
    assert_eq!(check_settle_timestamp(i64::MAX, i64::MAX), Ok(()));
}
