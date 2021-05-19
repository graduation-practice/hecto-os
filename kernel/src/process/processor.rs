//! 实现线程的调度和管理 [`Processor`]

use alloc::sync::Arc;

use algorithm::*;
use hashbrown::HashSet;
use lazy_static::*;

use super::*;
use crate::{arch::cpu::get_cpu_id, spinlock::*};

lazy_static! {
    /// 全局的 [`Processor`]，保存每 CPU 变量
    pub static ref PROCESSORS: [SpinLock<Processor>; 2] = [Default::default(), Default::default()];
}

lazy_static! {
    /// 调度器，保存 Ready 状态的线程
    pub static ref SCHEDULER: SpinLock<SchedulerImpl<Arc<Thread>>> = SpinLock::new(SchedulerImpl::default());
}

#[inline]
pub fn current_processor() -> &'static SpinLock<Processor> {
    &PROCESSORS[get_cpu_id()]
}

/// 每 cpu 变量
#[derive(Default)]
pub struct Processor {
    /// 保存休眠线程
    pub sleeping_threads: HashSet<Arc<Thread>>,
    pub idle_task_cx: usize,
}
