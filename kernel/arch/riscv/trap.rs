use core::intrinsics::transmute;

use riscv::register::{
    scause::{Exception, Interrupt, Scause, Trap},
    sepc,
    sstatus::{self, SPP},
    stvec,
};

use super::{cpu, timer};
use crate::{
    arch::{interface::Register, RegisterImpl, TrapFrameImpl},
    get_current_thread,
    syscall::syscall_handler,
    trap::handle_pagefault,
};

global_asm!(include_str!("./trap.asm"));
extern "C" {
    pub fn __trap();
    pub fn __restore();
}

pub struct TrapImpl;

impl crate::arch::interface::Trap for TrapImpl {
    fn init() {
        unsafe {
            // 使用 Direct 模式，将中断入口设置为 `__interrupt`
            stvec::write(__trap as usize, stvec::TrapMode::Direct);

            // // 开启 S 态外部中断
            // sie::set_sext();
            // // 开启 S 态软件中断
            // sie::set_ssoft();
        }

        // XXX
        timer::init();

        println!("mod trap initialized");
    }
}

/// 中断处理入口
#[no_mangle]
pub fn handle_trap(trap_frame: &mut TrapFrameImpl, scause: Scause, stval: usize) {
    debug_assert_eq!(
        unsafe { transmute::<_, usize>(trap_frame.sstatus) },
        unsafe { transmute::<_, usize>(sstatus::read()) }
    );
    debug_assert_eq!(trap_frame.sepc, sepc::read());
    debug_assert!(RegisterImpl::sp() >= 0xffff_ffc0_0000_0000);
    debug_assert!(
        {
            let spp = trap_frame.sstatus.spp();
            let spec = trap_frame.sepc;
            (spp == SPP::User && spec <= 0x0000_003f_ffff_ffff)
                || (spp == SPP::Supervisor && spec >= 0xffff_ffc0_0000_0000)
        },
        "spp = {:?}, sepc = {:x}",
        trap_frame.sstatus.spp(),
        trap_frame.sepc
    );

    match scause.cause() {
        // 时钟中断
        Trap::Interrupt(Interrupt::SupervisorTimer) => {
            timer::tick();
            if trap_frame.sstatus.spp() == SPP::User {
                print!("⏲️");
                // 抢占式调度
                get_current_thread().yield_to_sched();
            } else {
                print!("⏰");
            }
            return;
        }
        _ => {}
    }

    trace!("来自 {:?} 态的 trap", trap_frame.sstatus.spp());

    unsafe {
        // 开启 SIE（不是 sie 寄存器），全局中断使能，允许内核态被中断打断
        riscv::register::sstatus::set_sie();
    }

    match scause.cause() {
        // 来自用户态的系统调用
        Trap::Exception(Exception::UserEnvCall) => syscall_handler(),
        // 外部中断
        Trap::Interrupt(Interrupt::SupervisorExternal) => unimplemented!(),
        // 缺页异常
        Trap::Exception(Exception::LoadPageFault)
        | Trap::Exception(Exception::StorePageFault)
        | Trap::Exception(Exception::InstructionPageFault)
        | Trap::Exception(Exception::LoadFault)
        | Trap::Exception(Exception::StoreFault) => {
            warn!(
                "cause: {:?}, stval: {:x}, sepc: {:x}, sp = {:#x}",
                scause.cause(),
                stval,
                trap_frame.sepc,
                RegisterImpl::sp()
            );
            handle_pagefault(stval);
        }
        Trap::Exception(Exception::InstructionFault) => {
            #[cfg(feature = "k210")]
            panic!(
                "cause: Instruction access fault
            , stval: {:x}, sepc: {:x}",
                stval,
                sepc::read()
            );

            #[cfg(not(feature = "k210"))]
            panic!(
                "cause: {:?}, stval: {:x}, sepc: {:x}",
                scause.cause(),
                stval,
                sepc::read()
            );
        }
        // 其他情况，无法处理
        _ => {
            panic!(
                "cause: {:?}, stval: {:x}, sepc: {:x}",
                scause.cause(),
                stval,
                sepc::read()
            );
        }
    }

    unsafe {
        // 返回时关闭全局中断
        riscv::register::sstatus::clear_sie();
    }
    // println!("handle_interrupt end");
}

/// 用户线程第一次执行，经此函数进入 __restore。
/// XXX 修改此函数时需慎重！因为暂存 ra 的位置可能会发生改变。
/// 另外需注意 get_current_thread() 会被内联，对此函数的修改也会影响到这里
// #[no_mangle]
// #[inline(never)]
pub fn ret_to_restore() {
    get_current_thread().inner.critical_section(|inner| {
        // 线程第一次进入用户态的时刻
        inner.cycles = cpu::get_cycles();
    });

    let restore_va = __restore as usize;
    #[cfg(debug_assertions)]
    unsafe {
        llvm_asm!("sd $0, 56(sp)" :: "r"(restore_va) :: "volatile")
    };

    // XXX unsafe! 这里的 `8(sp)` 是函数暂存返回地址的地方
    #[cfg(not(debug_assertions))]
    unsafe {
        llvm_asm!("sd $0, 8(sp)" :: "r"(restore_va) :: "volatile")
    };
}
