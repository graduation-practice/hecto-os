use core::{fmt::Display, iter::Step, mem::size_of};

use crate::board::{interface::Config, ConfigImpl};

#[repr(C)]
#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq)]
// #[rustc_layout_scalar_valid_range_start(1)]
// #[rustc_nonnull_optimization_guaranteed]
pub struct PA(pub usize);

/// TODO impl Deref 和 DerefMut
#[repr(C)]
#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq)]
// #[rustc_layout_scalar_valid_range_start(1)]
// #[rustc_nonnull_optimization_guaranteed]
pub struct VA(pub usize);

#[repr(C)]
#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq)]
pub struct PPN(pub usize);

#[repr(C)]
#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq)]
pub struct VPN(pub usize);

/// 从指针转换为虚拟地址
impl<T> From<*const T> for VA {
    fn from(pointer: *const T) -> Self {
        Self(pointer as usize)
    }
}
/// 从指针转换为虚拟地址
impl<T> From<*mut T> for VA {
    fn from(pointer: *mut T) -> Self {
        Self(pointer as usize)
    }
}
impl<T> From<&T> for VA {
    fn from(pointer: &T) -> Self {
        Self(pointer as *const _ as usize)
    }
}
impl<T> From<&mut T> for VA {
    fn from(pointer: &mut T) -> Self {
        Self(pointer as *const _ as usize)
    }
}

/// 虚实页号之间的线性映射
impl From<PPN> for VPN {
    fn from(ppn: PPN) -> Self {
        Self(ppn.0 + (ConfigImpl::KERNEL_MAP_OFFSET >> ConfigImpl::PAGE_SIZE_BITS))
    }
}
/// 虚实页号之间的线性映射
impl From<VPN> for PPN {
    fn from(vpn: VPN) -> Self {
        Self(vpn.0 - (ConfigImpl::KERNEL_MAP_OFFSET >> ConfigImpl::PAGE_SIZE_BITS))
    }
}
/// 虚实地址之间的线性映射
impl From<PA> for VA {
    fn from(pa: PA) -> Self {
        Self(pa.0 + ConfigImpl::KERNEL_MAP_OFFSET)
    }
}
/// 虚实地址之间的线性映射
impl From<VA> for PA {
    fn from(va: VA) -> Self {
        Self(va.0 - ConfigImpl::KERNEL_MAP_OFFSET)
    }
}

macro_rules! implement_address_to_page_number {
    // 这里面的类型转换实现 [`From`] trait，会自动实现相反的 [`Into`] trait
    ($address_type: tt, $page_number_type: tt) => {
        /// 实现页号转地址
        impl From<$page_number_type> for $address_type {
            /// 从页号转换为地址
            fn from(page_number: $page_number_type) -> Self {
                Self(page_number.0 << ConfigImpl::PAGE_SIZE_BITS)
            }
        }

        impl $address_type {
            /// 将地址转换为页号，向下取整
            pub const fn floor(self) -> $page_number_type {
                $page_number_type(self.0 >> ConfigImpl::PAGE_SIZE_BITS)
            }

            /// 将地址转换为页号，向上取整
            pub const fn ceil(self) -> $page_number_type {
                $page_number_type(
                    (self.0 - 1 + ConfigImpl::PAGE_SIZE) >> ConfigImpl::PAGE_SIZE_BITS,
                )
            }

            /// 低 12 位的 offset
            pub const fn page_offset(&self) -> usize {
                self.0 & (ConfigImpl::PAGE_SIZE - 1)
            }
        }
    };
}
implement_address_to_page_number! {PA, PPN}
implement_address_to_page_number! {VA, VPN}

impl VPN {
    pub fn indexes(&self) -> [usize; 3] {
        let mut vpn = self.0;
        let mut idx = [0usize; 3];
        for i in (0..3).rev() {
            idx[i] = vpn & 511;
            vpn >>= 9;
        }
        idx
    }

    pub fn get_array<T>(&self) -> &'static mut [T] {
        assert!(ConfigImpl::PAGE_SIZE % size_of::<T>() == 0);
        unsafe {
            core::slice::from_raw_parts_mut(
                (self.0 << ConfigImpl::PAGE_SIZE_BITS) as *mut T,
                ConfigImpl::PAGE_SIZE / size_of::<T>(),
            )
        }
    }
}

impl VA {
    pub fn get_ref<T>(&self) -> &'static T {
        unsafe { &*(self.0 as *const T) }
    }

    pub fn get_mut<T>(&self) -> &'static mut T {
        unsafe { &mut *(self.0 as *mut T) }
    }

    pub fn as_mut<T>(&self) -> &'static mut T {
        unsafe { &mut *(self.0 as *mut T) }
    }

    pub fn write<T>(&self, src: T) {
        unsafe { *(self.0 as *mut T) = src };
    }

    pub fn as_ptr<T>(&self) -> *const T {
        self.0 as *const T
    }

    pub fn as_mut_ptr<T>(&self) -> *mut T {
        self.0 as *mut T
    }

    pub fn is_null(&self) -> bool {
        self.0 == 0
    }
}

/// 为各种仅包含一个 usize 的类型实现运算操作
/// TODO 把用不到的删掉
macro_rules! implement_usize_operations {
    ($type_name: ty) => {
        /// `+`
        #[allow(unused_unsafe)]
        impl core::ops::Add<usize> for $type_name {
            type Output = Self;

            fn add(self, other: usize) -> Self::Output {
                Self(self.0 + other)
            }
        }
        /// `+=`
        #[allow(unused_unsafe)]
        impl core::ops::AddAssign<usize> for $type_name {
            fn add_assign(&mut self, rhs: usize) {
                unsafe {
                    self.0 += rhs;
                }
            }
        }
        /// `-`
        #[allow(unused_unsafe)]
        impl core::ops::Sub<usize> for $type_name {
            type Output = Self;

            fn sub(self, other: usize) -> Self::Output {
                Self(self.0 - other)
            }
        }
        /// `-`
        impl core::ops::Sub<$type_name> for $type_name {
            type Output = usize;

            fn sub(self, other: $type_name) -> Self::Output {
                self.0 - other.0
            }
        }
        /// `-=`
        #[allow(unused_unsafe)]
        impl core::ops::SubAssign<usize> for $type_name {
            fn sub_assign(&mut self, rhs: usize) {
                self.0 -= rhs;
            }
        }
        /// 和 usize 相互转换
        #[allow(unused_unsafe)]
        impl From<usize> for $type_name {
            fn from(value: usize) -> Self {
                Self(value)
            }
        }
        /// 和 usize 相互转换
        impl From<$type_name> for usize {
            fn from(value: $type_name) -> Self {
                value.0
            }
        }
        /// 是否有效（0 为无效）
        impl $type_name {
            pub fn valid(&self) -> bool {
                self.0 != 0
            }
        }
        /// {} 输出
        impl core::fmt::Display for $type_name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, "{}(0x{:x})", stringify!($type_name), self.0)
            }
        }
    };
}
implement_usize_operations! {PA}
implement_usize_operations! {VA}
implement_usize_operations! {PPN}
implement_usize_operations! {VPN}

unsafe impl Step for VPN {
    fn steps_between(start: &Self, end: &Self) -> Option<usize> {
        Step::steps_between(&start.0, &end.0)
    }

    fn forward_checked(start: Self, count: usize) -> Option<Self> {
        Some(start + count)
    }

    fn backward_checked(start: Self, count: usize) -> Option<Self> {
        Some(start - count)
    }
}

unsafe impl Step for PPN {
    fn steps_between(start: &Self, end: &Self) -> Option<usize> {
        Step::steps_between(&start.0, &end.0)
    }

    fn forward_checked(start: Self, count: usize) -> Option<Self> {
        Some(start + count)
    }

    fn backward_checked(start: Self, count: usize) -> Option<Self> {
        Some(start - count)
    }
}

pub type VPNRange = core::ops::Range<VPN>;
pub type VARange = core::ops::Range<VA>;

/// 一个实现了 `Ord` Trait 的 VARange，还可以转为 VPNRange
#[derive(Clone)]
pub struct VARangeOrd(pub VARange);

impl VARangeOrd {
    /// 获取 VPNRange
    pub fn vpn_range(&self) -> VPNRange {
        self.0.start.floor()..self.0.end.ceil()
    }
}

impl Ord for VARangeOrd {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        if self.eq(other) {
            core::cmp::Ordering::Equal
        } else if self.0.start < other.0.start {
            core::cmp::Ordering::Less
        } else {
            core::cmp::Ordering::Greater
        }
    }
}
impl PartialOrd for VARangeOrd {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Eq for VARangeOrd {}
impl PartialEq for VARangeOrd {
    fn eq(&self, other: &Self) -> bool {
        (self.0.start <= other.0.start && other.0.end <= self.0.end)
            || (other.0.start <= self.0.start && self.0.end <= other.0.end)
    }
}

impl Display for VARangeOrd {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "[{}, {})", self.0.start, self.0.end)
    }
}

#[macro_export]
macro_rules! round_down {
    ($value: expr, $boundary: expr) => {
        ($value as usize & !($boundary - 1))
    };
}

#[macro_export]
macro_rules! round_up {
    ($value: expr, $boundary: expr) => {
        ($value as usize + $boundary - 1 & !($boundary - 1))
    };
}

#[cfg(test)]
mod tests {
    use test_macros::kernel_test;

    use super::*;

    #[kernel_test]
    fn test_display() {
        println!("{}", VA(0x12345));
        println!("{}", VARangeOrd(VA(0x12345)..VA(0x12345)));
    }
}
