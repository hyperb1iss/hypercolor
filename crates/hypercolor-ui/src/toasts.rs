//! Toast notification helpers — thin wrappers around leptoaster.

use leptoaster::expect_toaster;

pub fn toast_success(msg: &str) {
    expect_toaster().success(msg);
}

pub fn toast_error(msg: &str) {
    expect_toaster().error(msg);
}

pub fn toast_info(msg: &str) {
    expect_toaster().info(msg);
}
