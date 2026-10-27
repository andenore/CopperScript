//! Backend-neutral physical board geometry (`pcbir.physical`, `pcbir.geometry`,
//! `pcbir.mechanical`, `pcbir.mechanical_curves`).
//!
//! Coordinates are integer nanometres. Predicates are exact: products widen
//! to `i128`, and the few that can exceed it fall back to big integers, so a
//! query never answers differently from the Python oracle's unbounded ints.
//! Constructors validate like the Python dataclasses and return the
//! `ValueError` message on failure.
//!
//! Lengths are limited to [`MAX_COORDINATE`]; the frontend refuses larger
//! values as not implemented rather than risk an overflow.

pub mod board;
pub mod curves;
pub mod geometry;
pub mod mechanical;
pub mod pyfloat;

pub use board::*;
pub use geometry::{Bounds, Point, RoundedConvexShape};

/// The largest coordinate magnitude (2^40 nm, about 1.1 km) the geometry accepts.
pub const MAX_COORDINATE: i64 = 1 << 40;
