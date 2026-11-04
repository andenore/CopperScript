//! Native manufacturing artwork without KiCad: Gerber X2 layers, Excellon
//! drills, an IPC-D-356A netlist and component positions plotted from a
//! (filled) `.kicad_pcb`, plus a CAM comparison of two such file sets that
//! rasterises every Gerber layer and matches drill hits, netlist records and
//! positions.
//!
//! Zone fills are read from the board as saved by a refill; this crate does
//! not fill zones.

pub mod board;
pub mod compare;
pub mod font;
pub mod geometry;
pub mod gerber;
pub mod plot;
