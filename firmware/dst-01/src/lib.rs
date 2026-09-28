#![no_std]

#[cfg(all(feature = "host", not(any(feature = "rev1", feature = "rev2"))))]
compile_error!("host builds must select rev1 or rev2");
#[cfg(all(feature = "host", feature = "rev1", feature = "rev2"))]
compile_error!("rev1 and rev2 are mutually exclusive");

pub mod district;
pub mod network;

#[cfg(all(
    feature = "host",
    any(feature = "rev1", feature = "rev2"),
    not(all(feature = "rev1", feature = "rev2"))
))]
pub mod host;

#[cfg(target_os = "none")]
pub mod tasks;
