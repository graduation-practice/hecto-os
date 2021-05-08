//! 中断模块
//!
//!

mod context;
mod handler;
mod timer;

pub use context::Context;

/// 初始化中断相关的子模块
///
/// - [`handler::init`]
/// - [`timer::init`]
pub fn init() {
    handler::init();
    timer::init();

    log::info!("mod interrupt initialized");
}

extern "C" {
    pub fn __interrupt();
    pub fn __restore();
}
