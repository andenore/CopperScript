//! Native manufacturing artwork without KiCad: Gerber X2 layers, Excellon
//! drills, an IPC-D-356A netlist and component positions plotted from a
//! (filled) `.kicad_pcb`, plus a CAM comparison of two such file sets that
//! rasterises every Gerber layer and matches drill hits, netlist records and
//! positions.
//!
//! Zones are filled natively ([`fill`], with the design rules of [`rules`])
//! and written back into the board ([`pcb_write`]); the plotter reads the
//! fills saved in the board, natively or by a KiCad refill.

pub mod board;
pub mod compare;
pub mod fill;
pub mod font;
pub mod geometry;
pub mod gerber;
pub mod pcb_write;
pub mod plot;
pub mod poly;
pub mod rules;
