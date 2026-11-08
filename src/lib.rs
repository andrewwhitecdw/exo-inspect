//! Inspect ExodusII files (MOOSE `ExodusII` output) without ParaView.
//!
//! The crate has two layers: [`netcdf`] implements the classic netCDF
//! container format (CDF-1/CDF-2) from scratch, and [`exodus`] interprets
//! the ExodusII conventions (element blocks, sets, variable names) on top.
//! The binary in `src/main.rs` is a thin wrapper.

pub mod exodus;
pub mod netcdf;
